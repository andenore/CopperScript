//! `physical_board_digest`: a digest of every geometry and rule that can
//! affect manufacturing signoff.
//!
//! The Python document embeds the dataclass `repr` of many IR records; the
//! functions here spell those reprs out field by field, in declaration order.

use sha2::{Digest, Sha256};

use copper_json::{Dumps, Value as Json, python_str_repr as s};
use copper_pcb::board::*;
use copper_physical::footprint::{FootprintGraphic, FootprintLayer, PhysicalFootprint};
use copper_physical::{
    BoardCutout, BoardOutline, BoardSide, BoundarySegment, CopperKeepout, CopperLayer, PlacementKeepout, Point,
};
use copper_units::Decimal;

/// A dataclass repr: `Name(field=value, ...)`.
fn dc(name: &str, fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields.iter().map(|(key, value)| format!("{key}={value}")).collect();
    format!("{name}({})", body.join(", "))
}

/// A tuple repr, with the one-element trailing comma.
fn tuple(items: Vec<String>) -> String {
    match items.len() {
        1 => format!("({},)", items[0]),
        _ => format!("({})", items.join(", ")),
    }
}

fn int(value: i64) -> String {
    value.to_string()
}

fn opt_int(value: Option<i64>) -> String {
    value.map_or_else(|| "None".to_owned(), int)
}

fn opt_str(value: Option<&str>) -> String {
    value.map_or_else(|| "None".to_owned(), s)
}

fn boolean(value: bool) -> String {
    if value { "True" } else { "False" }.to_owned()
}

fn decimal(value: &Decimal) -> String {
    format!("Decimal({})", s(&value.to_sci_string()))
}

fn enum_repr(class: &str, member: &str, value: &str) -> String {
    format!("<{class}.{member}: {}>", s(value))
}

fn upper(text: &str) -> String {
    let mut result = String::new();
    for (index, c) in text.chars().enumerate() {
        if c.is_ascii_uppercase() && index > 0 {
            result.push('_');
        }
        result.push(c.to_ascii_uppercase());
    }
    result
}

fn layer(value: CopperLayer) -> String {
    let member = match value {
        CopperLayer::Front => "FRONT".to_owned(),
        CopperLayer::Back => "BACK".to_owned(),
        other => format!("INTERNAL_{}", other.value().trim_start_matches("In").trim_end_matches(".Cu")),
    };
    enum_repr("CopperLayer", &member, value.value())
}

fn layers(values: &[CopperLayer]) -> String {
    tuple(values.iter().map(|l| layer(*l)).collect())
}

fn side(value: BoardSide) -> String {
    enum_repr("BoardSide", &upper(value.value()), value.value())
}

fn opt_side(value: Option<BoardSide>) -> String {
    value.map_or_else(|| "None".to_owned(), side)
}

fn point(p: Point) -> String {
    dc("Point", &[("x_nm", int(p.x_nm)), ("y_nm", int(p.y_nm))])
}

fn points(values: &[Point]) -> String {
    tuple(values.iter().map(|p| point(*p)).collect())
}

fn pad(reference: &PadReference) -> String {
    dc("PadReference", &[("component", s(&reference.component)), ("pad", s(&reference.pad))])
}

fn pads(values: &[PadReference]) -> String {
    tuple(values.iter().map(pad).collect())
}

fn ring(vertices: &[Point]) -> String {
    dc("PolygonRing", &[("vertices", points(vertices))])
}

fn polygon_with_holes(outer: &[Point], holes: &[Vec<Point>]) -> String {
    dc("PolygonWithHoles", &[("outer", ring(outer)), ("holes", tuple(holes.iter().map(|h| ring(h)).collect()))])
}

fn outline(value: &BoardOutline) -> String {
    let cutout = |c: &BoardCutout| dc("BoardCutout", &[("id", s(&c.id)), ("vertices", points(&c.vertices))]);
    dc(
        "BoardOutline",
        &[
            ("vertices", points(&value.vertices)),
            ("cutouts", tuple(value.cutouts.iter().map(cutout).collect())),
            ("circular_boundary", circular_boundary(value)),
            ("boundary_path", boundary_path(value)),
        ],
    )
}

fn circular_boundary(value: &BoardOutline) -> String {
    match &value.circular_boundary {
        None => "None".to_owned(),
        Some(circle) => dc(
            "CircularBoardBoundary",
            &[
                ("center", point(circle.center)),
                ("radius_nm", int(circle.radius_nm)),
                ("maximum_chord_error_nm", int(circle.maximum_chord_error_nm)),
            ],
        ),
    }
}

fn boundary_path(value: &BoardOutline) -> String {
    match &value.boundary_path {
        None => "None".to_owned(),
        Some(path) => {
            let segments = path
                .segments
                .iter()
                .map(|segment| match segment {
                    BoundarySegment::Line { id, start, end } => {
                        dc("BoundaryLine", &[("id", s(id)), ("start", point(*start)), ("end", point(*end))])
                    }
                    BoundarySegment::Arc { id, start, mid, end } => dc(
                        "BoundaryArc",
                        &[("id", s(id)), ("start", point(*start)), ("mid", point(*mid)), ("end", point(*end))],
                    ),
                })
                .collect();
            dc(
                "BoardBoundaryPath",
                &[("segments", tuple(segments)), ("maximum_chord_error_nm", int(path.maximum_chord_error_nm))],
            )
        }
    }
}

fn copper_keepout(keepout: &CopperKeepout) -> String {
    dc(
        "CopperKeepout",
        &[
            ("id", s(&keepout.id)),
            ("layers", layers(&keepout.layers)),
            ("outline", polygon_with_holes(&keepout.vertices, &[])),
            ("block_tracks", boolean(keepout.block_tracks)),
            ("block_vias", boolean(keepout.block_vias)),
            ("block_pads", boolean(keepout.block_pads)),
            ("block_zones", boolean(keepout.block_zones)),
            ("block_footprints", boolean(keepout.block_footprints)),
        ],
    )
}

fn placement_keepout(keepout: &PlacementKeepout) -> String {
    dc(
        "PlacementKeepout",
        &[
            ("name", s(&keepout.name)),
            ("outline", outline(&keepout.outline)),
            ("side", opt_side(keepout.side)),
            ("maximum_component_height_nm", opt_int(keepout.maximum_component_height_nm)),
        ],
    )
}

fn track(item: &TrackSegment) -> String {
    dc(
        "TrackSegment",
        &[
            ("net", s(&item.net)),
            ("start", point(item.start)),
            ("end", point(item.end)),
            ("width_nm", int(item.width_nm)),
            ("layer", layer(item.layer)),
        ],
    )
}

fn via(item: &Via) -> String {
    dc(
        "Via",
        &[
            ("net", s(&item.net)),
            ("position", point(item.position)),
            ("size_nm", int(item.size_nm)),
            ("drill_nm", int(item.drill_nm)),
            ("from_layer", layer(item.from_layer)),
            ("to_layer", layer(item.to_layer)),
            ("technology", opt_str(item.technology.as_deref())),
            ("finish", s(&item.finish)),
        ],
    )
}

fn zone(item: &CopperZone) -> String {
    let connection = item.pad_connection.value();
    let policy = item.island_policy.value();
    dc(
        "CopperZone",
        &[
            ("id", s(&item.id)),
            ("net", s(&item.net)),
            ("layers", layers(&item.layers)),
            ("outline", polygon_with_holes(&item.outer, &item.holes)),
            ("priority", int(item.priority)),
            ("clearance_nm", opt_int(item.clearance_nm)),
            ("minimum_width_nm", int(item.minimum_width_nm)),
            ("pad_connection", enum_repr("ZoneConnection", &connection.to_ascii_uppercase(), connection)),
            (
                "thermal",
                dc(
                    "ThermalReliefSettings",
                    &[
                        ("gap_nm", int(item.thermal_gap_nm)),
                        ("spoke_width_nm", int(item.thermal_spoke_width_nm)),
                        ("spoke_count", int(4)),
                    ],
                ),
            ),
            ("island_policy", enum_repr("IslandPolicy", &policy.to_ascii_uppercase(), policy)),
            ("minimum_island_area_nm2", opt_int(item.minimum_island_area_nm2)),
            ("fill_mode", enum_repr("ZoneFillMode", "SOLID", "solid")),
        ],
    )
}

fn copper_polygon(item: &CopperPolygon) -> String {
    dc(
        "CopperPolygon",
        &[("id", s(&item.id)), ("net", s(&item.net)), ("layer", layer(item.layer)), ("outline", ring(&item.vertices))],
    )
}

fn plane_return(item: &MacroPlaneReturn) -> String {
    let contacts = item.dedicated_contacts.iter().map(|(p, at)| tuple(vec![pad(p), point(*at)])).collect();
    dc(
        "MacroPlaneReturn",
        &[
            ("net", s(&item.net)),
            ("layers", layers(&item.layers)),
            ("pads", pads(&item.pads)),
            ("dedicated_contacts", tuple(contacts)),
        ],
    )
}

fn hard_macro(item: &PhysicalHardMacro) -> String {
    let ports = item
        .ports
        .iter()
        .map(|port| {
            dc(
                "MacroPort",
                &[
                    ("name", s(&port.name)),
                    ("net", s(&port.net)),
                    ("position", point(port.position)),
                    ("layer", layer(port.layer)),
                    ("pads", pads(&port.pads)),
                ],
            )
        })
        .collect();
    let bindings =
        item.pad_bindings.iter().map(|(p, net)| dc("MacroPadBinding", &[("pad", pad(p)), ("net", s(net))])).collect();
    let contracts = item
        .width_contracts
        .iter()
        .map(|c| {
            dc(
                "MacroTrackWidthContract",
                &[
                    ("track_index", c.track_index.to_string()),
                    ("minimum_width_nm", int(c.minimum_width_nm)),
                    ("purpose", s(&c.purpose)),
                    ("evidence", s(&c.evidence)),
                ],
            )
        })
        .collect();
    dc(
        "PhysicalHardMacro",
        &[
            ("cluster", s(&item.cluster)),
            ("asset_sha256", s(&item.asset_sha256)),
            ("tracks", tuple(item.tracks.iter().map(track).collect())),
            ("vias", tuple(item.vias.iter().map(via).collect())),
            ("ports", tuple(ports)),
            ("protected_regions", tuple(item.protected_regions.iter().map(copper_keepout).collect())),
            ("required_layers", layers(&item.required_layers)),
            ("pad_bindings", tuple(bindings)),
            ("isolated_pads", pads(&item.isolated_pads)),
            ("width_contracts", tuple(contracts)),
        ],
    )
}

fn target(item: &PlacementTarget) -> String {
    dc("PlacementTarget", &[("reference", s(&item.reference)), ("pad", opt_str(item.pad.as_deref()))])
}

fn rigid_cluster(item: &RigidPlacementCluster) -> String {
    let members = item
        .members
        .iter()
        .map(|m| {
            dc(
                "RigidPlacementMember",
                &[
                    ("reference", s(&m.reference)),
                    ("footprint", s(&m.footprint)),
                    ("footprint_digest", s(&m.footprint_digest)),
                    ("position", point(m.position)),
                    ("rotation_degrees", decimal(&m.rotation_degrees)),
                ],
            )
        })
        .collect();
    let openings = item
        .edge_openings
        .iter()
        .map(|o| {
            dc(
                "MacroEdgeOpening",
                &[
                    ("keepout", s(&o.keepout)),
                    ("edge_indices", tuple(o.edge_indices.iter().map(|i| int(*i)).collect())),
                ],
            )
        })
        .collect();
    dc(
        "RigidPlacementCluster",
        &[
            ("name", s(&item.name)),
            (
                "anchor",
                target(&PlacementTarget { reference: item.anchor_reference.clone(), pad: item.anchor_pad.clone() }),
            ),
            ("members", tuple(members)),
            ("source", s(&item.source)),
            ("allowed_rotations", tuple(item.allowed_rotations.iter().map(decimal).collect())),
            ("keepouts", tuple(item.keepouts.iter().map(copper_keepout).collect())),
            ("internal_clearance_nm", opt_int(item.internal_clearance_nm)),
            ("edge_openings", tuple(openings)),
        ],
    )
}

fn placement_rule(rule: &ComponentPlacementRule) -> String {
    dc(
        "ComponentPlacementRule",
        &[
            ("reference", s(&rule.reference)),
            ("region", opt_str(rule.region.as_deref())),
            ("allowed_orientations", tuple(rule.allowed_orientations.iter().map(decimal).collect())),
            ("fixed_position", rule.fixed_position.map_or_else(|| "None".to_owned(), point)),
            ("fixed_rotation_degrees", rule.fixed_rotation_degrees.as_ref().map_or_else(|| "None".to_owned(), decimal)),
            ("side", opt_side(rule.side)),
            ("priority", int(rule.priority)),
            ("edge_clearance_nm", opt_int(rule.edge_clearance_nm)),
            ("additional_courtyard_gap_nm", opt_int(rule.additional_courtyard_gap_nm)),
        ],
    )
}

fn relative_rule(rule: &RelativePlacementRule) -> String {
    let kind = rule.kind.value();
    dc(
        "RelativePlacementRule",
        &[
            ("kind", enum_repr("RelativePlacementKind", &kind.to_ascii_uppercase(), kind)),
            ("targets", tuple(rule.targets.iter().map(target).collect())),
            ("distance_nm", opt_int(rule.distance_nm)),
            (
                "axis",
                rule.axis.map_or_else(
                    || "None".to_owned(),
                    |a| enum_repr("AlignmentAxis", &a.value().to_ascii_uppercase(), a.value()),
                ),
            ),
            ("tolerance_nm", int(rule.tolerance_nm)),
            ("weight", int(rule.weight)),
        ],
    )
}

fn placement_group(group: &PlacementGroup) -> String {
    dc(
        "PlacementGroup",
        &[
            ("name", s(&group.name)),
            ("references", tuple(group.references.iter().map(|r| s(r)).collect())),
            ("anchor", opt_str(group.anchor.as_deref())),
            ("priority", int(group.priority)),
            ("source", s(&group.source)),
        ],
    )
}

fn region(item: &PlacementRegion) -> String {
    dc(
        "PlacementRegion",
        &[("name", s(&item.name)), ("outline", outline(&item.outline)), ("side", opt_side(item.side))],
    )
}

fn graphic(item: &FootprintGraphic) -> String {
    let footprint_layer = |l: FootprintLayer| enum_repr("FootprintLayer", &l.value().to_ascii_uppercase(), l.value());
    match item {
        FootprintGraphic::Line { start, end, width_nm, layer } => dc(
            "FootprintLine",
            &[
                ("start", point(*start)),
                ("end", point(*end)),
                ("width_nm", int(*width_nm)),
                ("layer", footprint_layer(*layer)),
            ],
        ),
        FootprintGraphic::Rectangle { start, end, width_nm, layer, filled } => dc(
            "FootprintRectangle",
            &[
                ("start", point(*start)),
                ("end", point(*end)),
                ("width_nm", int(*width_nm)),
                ("layer", footprint_layer(*layer)),
                ("filled", boolean(*filled)),
            ],
        ),
        FootprintGraphic::Circle { center, end, width_nm, layer, filled } => dc(
            "FootprintCircle",
            &[
                ("center", point(*center)),
                ("end", point(*end)),
                ("width_nm", int(*width_nm)),
                ("layer", footprint_layer(*layer)),
                ("filled", boolean(*filled)),
            ],
        ),
        FootprintGraphic::Arc { start, midpoint, end, width_nm, layer } => dc(
            "FootprintArc",
            &[
                ("start", point(*start)),
                ("midpoint", point(*midpoint)),
                ("end", point(*end)),
                ("width_nm", int(*width_nm)),
                ("layer", footprint_layer(*layer)),
            ],
        ),
        FootprintGraphic::Polygon { points: vertices, width_nm, layer, filled } => dc(
            "FootprintPolygon",
            &[
                ("points", points(vertices)),
                ("width_nm", int(*width_nm)),
                ("layer", footprint_layer(*layer)),
                ("filled", boolean(*filled)),
            ],
        ),
    }
}

fn j_int(value: i64) -> Json {
    Json::Int(value.into())
}

fn j_opt(value: Option<i64>) -> Json {
    value.map_or(Json::Null, j_int)
}

fn j_str(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn j_list(items: Vec<Json>) -> Json {
    Json::Array(items)
}

fn j_point(p: Point) -> Json {
    j_list(vec![j_int(p.x_nm), j_int(p.y_nm)])
}

fn reprs<T>(items: &[T], key: impl Fn(&T) -> String, repr: impl Fn(&T) -> String) -> Json {
    let mut sorted: Vec<&T> = items.iter().collect();
    sorted.sort_by_key(|item| key(item));
    j_list(sorted.into_iter().map(|item| Json::Str(repr(item))).collect())
}

/// `_footprint_document`.
fn footprint_document(name: &str, footprint: &PhysicalFootprint) -> Json {
    let pads = footprint
        .pads
        .iter()
        .map(|pad| {
            j_list(vec![
                j_str(&pad.number),
                j_int(pad.position.x_nm),
                j_int(pad.position.y_nm),
                j_int(pad.size.width_nm),
                j_int(pad.size.height_nm),
                j_str(pad.kind.value()),
                j_str(pad.shape.value()),
                j_str(&pad.rotation_degrees.to_sci_string()),
                pad.drill.map_or(Json::Null, |d| j_list(vec![j_int(d.width_nm), j_int(d.height_nm)])),
                j_int(pad.roundrect_ratio_ppm),
                Json::Bool(pad.has_solder_mask),
                Json::Bool(pad.has_solder_paste),
                pad.zone_connection.map_or(Json::Null, |z| j_str(z.value())),
                Json::Bool(pad.heatsink),
                Json::Bool(pad.remove_unused_layers),
                Json::Bool(pad.connector_contact),
            ])
        })
        .collect();
    let mut metadata: Vec<(&String, &String)> = footprint.metadata.iter().collect();
    metadata.sort();
    j_list(vec![
        j_str(name),
        footprint.source_library_id.as_deref().map_or(Json::Null, j_str),
        j_list(
            footprint
                .internal_pad_groups
                .iter()
                .map(|g| j_list(g.numbers.iter().map(|n| j_str(n)).collect()))
                .collect(),
        ),
        j_list(vec![j_int(footprint.body_size.width_nm), j_int(footprint.body_size.height_nm)]),
        j_list(pads),
        j_list(footprint.graphics.iter().map(|g| Json::Str(graphic(g))).collect()),
        j_list(footprint.keepouts.iter().map(|k| Json::Str(copper_keepout(k))).collect()),
        j_list(footprint.courtyard.iter().map(|p| j_point(*p)).collect()),
        j_list(metadata.into_iter().map(|(k, v)| j_list(vec![j_str(k), j_str(v)])).collect()),
        j_opt(footprint.height_nm),
        j_opt(footprint.clearance_nm),
        Json::Bool(footprint.exclude_from_bom),
        Json::Bool(footprint.exclude_from_pos_files),
    ])
}

/// `_routing_rule_document`.
fn routing_rule_document(rule: &NetRoutingRule) -> Json {
    let mut document = vec![
        j_str(&rule.net),
        j_str(rule.kind.value()),
        j_int(rule.priority),
        j_opt(rule.width_nm),
        j_opt(rule.clearance_nm),
        j_list(rule.allowed_layers.iter().map(|l| j_str(l.value())).collect()),
        j_opt(rule.max_vias),
        j_opt(rule.max_length_nm),
        rule.differential_partner.as_deref().map_or(Json::Null, j_str),
        j_opt(rule.pair_gap_nm),
        j_opt(rule.max_skew_nm),
        j_str(&rule.topology),
        j_opt(rule.target_impedance_ohms),
        j_opt(rule.maximum_uncoupled_length_nm),
        j_opt(rule.maximum_stub_length_nm),
        j_opt(rule.tuning_amplitude_limit_nm),
        Json::Bool(rule.require_return_vias),
        Json::Bool(rule.reserve_corridor),
        rule.return_via_net.as_deref().map_or(Json::Null, j_str),
        j_opt(rule.maximum_return_via_distance_nm),
        rule.impedance_evidence_digest.as_deref().map_or(Json::Null, j_str),
        j_str(rule.return_via_policy.value()),
        rule.shared_reference_layer.map_or(Json::Null, |l| j_str(l.value())),
    ];
    let signal = [
        j_opt(rule.target_single_ended_ohms),
        rule.impedance_tolerance_percent.as_ref().map_or(Json::Null, |t| j_str(&t.to_sci_string())),
        rule.layer_group.as_deref().map_or(Json::Null, j_str),
        j_opt(rule.breakout_length_nm),
        j_opt(rule.breakout_width_nm),
        j_opt(rule.breakout_gap_nm),
        j_opt(rule.breakout_clearance_nm),
    ];
    if signal.iter().any(|value| *value != Json::Null) {
        document.push(j_list(signal.to_vec()));
    }
    if rule.tuning_style != TuningStyle::Bumps || rule.tuning_spacing_nm.is_some() {
        document.push(j_list(vec![j_str("tuning"), j_str(rule.tuning_style.value()), j_opt(rule.tuning_spacing_nm)]));
    }
    if let Some(group) = &rule.tuning_group {
        document.push(j_list(vec![j_str("tuning_group"), j_str(group)]));
    }
    j_list(document)
}

fn track_identity(item: &TrackSegment) -> (String, String, (i64, i64), (i64, i64), i64) {
    let mut endpoints = [(item.start.x_nm, item.start.y_nm), (item.end.x_nm, item.end.y_nm)];
    endpoints.sort();
    (item.net.clone(), item.layer.value().to_owned(), endpoints[0], endpoints[1], item.width_nm)
}

type ViaIdentity = (String, i64, i64, String, String, i64, i64, Option<String>, String);

fn via_identity(item: &Via) -> ViaIdentity {
    (
        item.net.clone(),
        item.position.x_nm,
        item.position.y_nm,
        item.from_layer.value().to_owned(),
        item.to_layer.value().to_owned(),
        item.size_nm,
        item.drill_nm,
        item.technology.clone(),
        item.finish.clone(),
    )
}

fn stackup_layer(item: &copper_lang::mechanical::stackup::StackupLayer) -> String {
    let kind = if item.copper_layer.is_some() { ("COPPER", "copper") } else { ("DIELECTRIC", "dielectric") };
    let opt_decimal = |value: &Option<Decimal>| value.as_ref().map_or_else(|| "None".to_owned(), decimal);
    dc(
        "StackupLayer",
        &[
            ("id", s(&item.id)),
            ("kind", enum_repr("StackupLayerKind", kind.0, kind.1)),
            ("thickness_nm", int(item.thickness_nm)),
            ("copper_layer", item.copper_layer.map_or_else(|| "None".to_owned(), layer)),
            ("material", opt_str(item.material.as_deref())),
            ("relative_permittivity", opt_decimal(&item.relative_permittivity)),
            ("loss_tangent", opt_decimal(&item.loss_tangent)),
            ("dielectric_type", opt_str(item.dielectric_type.as_deref())),
        ],
    )
}

/// The `physical_board_digest` document.
pub fn board_document(board: &PhysicalBoard) -> Json {
    let mut document: Vec<(&str, Json)> = vec![("name", j_str(&board.name))];
    if !board.silkscreen.is_empty() {
        document.push(("silkscreen", copper_lang::silkscreen::artwork_document(&board.silkscreen)));
    }
    let outline_vertices = j_list(board.outline.vertices.iter().map(|p| j_point(*p)).collect());
    document.push(("outline", outline_vertices));
    document.push(("circular_boundary", Json::Str(circular_boundary(&board.outline))));
    document.push(("boundary_path", Json::Str(boundary_path(&board.outline))));
    document.push((
        "cutouts",
        j_list(
            board
                .outline
                .cutouts
                .iter()
                .map(|c| j_list(vec![j_str(&c.id), j_list(c.vertices.iter().map(|p| j_point(*p)).collect())]))
                .collect(),
        ),
    ));
    document.push((
        "mechanical_holes",
        reprs(
            &board.mechanical_holes,
            |h| h.id.clone(),
            |h| {
                dc(
                    "MechanicalHole",
                    &[
                        ("id", s(&h.id)),
                        ("position", point(h.position)),
                        ("diameter_nm", int(h.diameter_nm)),
                        ("head_clearance_radius_nm", int(h.head_clearance_radius_nm)),
                    ],
                )
            },
        ),
    ));
    document.push((
        "mechanical_slots",
        reprs(
            &board.mechanical_slots,
            |h| h.id.clone(),
            |h| {
                dc(
                    "MechanicalSlot",
                    &[
                        ("id", s(&h.id)),
                        ("start", point(h.start)),
                        ("end", point(h.end)),
                        ("width_nm", int(h.width_nm)),
                    ],
                )
            },
        ),
    ));
    let mut references: Vec<&copper_lang::mechanical::MechanicalReference> =
        board.mechanical_references.iter().collect();
    references.sort_by(|a, b| a.id.cmp(&b.id));
    document.push((
        "mechanical_references",
        j_list(references.into_iter().map(copper_lang::mechanical::reference_scene).collect()),
    ));
    document.push((
        "datums",
        reprs(
            &board.datums,
            |d| d.id.clone(),
            |d| {
                dc(
                    "BoardDatum",
                    &[
                        ("id", s(&d.id)),
                        ("position", point(d.position)),
                        ("relative_to", opt_str(d.relative_to.as_deref())),
                        ("offset", point(d.offset)),
                    ],
                )
            },
        ),
    ));
    document.push((
        "boundary_edges",
        reprs(
            &board.boundary_edges,
            |e| e.id.clone(),
            |e| dc("BoardEdge", &[("id", s(&e.id)), ("start", point(e.start)), ("end", point(e.end))]),
        ),
    ));
    document.push((
        "attachments",
        reprs(
            &board.attachments,
            |a| a.id.clone(),
            |a| {
                dc(
                    "PhysicalAttachment",
                    &[
                        ("id", s(&a.id)),
                        ("reference", s(&a.reference)),
                        ("target", opt_str(a.target.as_deref())),
                        ("position", point(a.position)),
                        ("offset", point(a.offset)),
                        ("anchor", s(&a.anchor)),
                        ("anchor_point", point(a.anchor_point)),
                        ("rotation", decimal(&a.rotation)),
                        ("side", side(a.side)),
                    ],
                )
            },
        ),
    ));
    document.push((
        "body_overhangs",
        reprs(
            &board.body_overhangs,
            |a| a.id.clone(),
            |a| {
                dc(
                    "BodyOverhang",
                    &[
                        ("id", s(&a.id)),
                        ("reference", s(&a.reference)),
                        ("edge", s(&a.edge)),
                        ("start_nm", int(a.start_nm)),
                        ("end_nm", int(a.end_nm)),
                        ("distance_nm", int(a.distance_nm)),
                        ("reason", s(&a.reason)),
                    ],
                )
            },
        ),
    ));
    document.push((
        "component_heights",
        reprs(
            &board.component_heights,
            |a| a.id.clone(),
            |a| {
                dc(
                    "ComponentHeight",
                    &[("id", s(&a.id)), ("reference", s(&a.reference)), ("height_nm", int(a.height_nm))],
                )
            },
        ),
    ));
    document.push((
        "assembly_envelopes",
        reprs(
            &board.assembly_envelopes,
            |a| a.id.clone(),
            |a| {
                dc(
                    "AssemblyEnvelope",
                    &[
                        ("id", s(&a.id)),
                        ("outline", outline(&a.outline)),
                        ("side", side(a.side)),
                        ("maximum_height_nm", int(a.maximum_height_nm)),
                    ],
                )
            },
        ),
    ));
    document.push((
        "assembly_access",
        reprs(
            &board.assembly_access,
            |a| a.id.clone(),
            |a| {
                dc(
                    "AssemblyAccess",
                    &[
                        ("id", s(&a.id)),
                        ("reference", s(&a.reference)),
                        ("outline", outline(&a.outline)),
                        ("side", s(&a.side)),
                        ("purpose", s(&a.purpose)),
                    ],
                )
            },
        ),
    ));
    let stackup = &board.stackup;
    document.push((
        "stackup",
        Json::Object(
            [
                ("layers", j_list(stackup.copper_layers.iter().map(|l| j_str(l.value())).collect())),
                ("thickness_nm", j_int(stackup.thickness_nm)),
                (
                    "physical_layers",
                    j_list(stackup.physical_layers.iter().map(|l| Json::Str(stackup_layer(l))).collect()),
                ),
                ("via_technologies", j_list(Vec::new())),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        ),
    ));
    let rules = &board.rules;
    let mut rule_entries = vec![
        ("minimum_slot_width", j_int(rules.minimum_slot_width_nm)),
        ("clearance", j_int(rules.minimum_clearance_nm)),
        ("hole_clearance", j_int(rules.minimum_hole_clearance_nm)),
        ("minimum_track_width", j_int(rules.minimum_track_width_nm)),
        ("track", j_int(rules.default_track_width_nm)),
        ("via", j_int(rules.default_via_size_nm)),
        ("drill", j_int(rules.default_via_drill_nm)),
    ];
    if let Some(value) = rules.minimum_via_size_nm {
        rule_entries.push(("minimum_via", j_int(value)));
    }
    if let Some(value) = rules.minimum_via_drill_nm {
        rule_entries.push(("minimum_via_drill", j_int(value)));
    }
    document.push(("rules", Json::Object(rule_entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())));
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    document.push((
        "placements",
        j_list(
            placements
                .into_iter()
                .map(|p| {
                    j_list(vec![
                        j_str(&p.reference),
                        j_str(&p.footprint),
                        j_int(p.position.x_nm),
                        j_int(p.position.y_nm),
                        j_str(&p.rotation_degrees.to_sci_string()),
                        j_str(p.side.value()),
                    ])
                })
                .collect(),
        ),
    ));
    let mut footprints: Vec<(&String, &PhysicalFootprint)> = board.footprints.iter().collect();
    footprints.sort_by(|a, b| a.0.cmp(b.0));
    document.push(("footprints", j_list(footprints.into_iter().map(|(n, f)| footprint_document(n, f)).collect())));
    let mut nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    document.push((
        "nets",
        j_list(
            nets.into_iter()
                .map(|net| {
                    let mut pads: Vec<&PadReference> = net.pads.iter().collect();
                    pads.sort();
                    j_list(vec![
                        j_str(&net.name),
                        j_list(pads.into_iter().map(|p| j_list(vec![j_str(&p.component), j_str(&p.pad)])).collect()),
                    ])
                })
                .collect(),
        ),
    ));
    if !board.power_feeds.is_empty() {
        let feeds = board
            .power_feeds
            .iter()
            .map(|f| {
                Json::Str(dc(
                    "PowerFeedBoundary",
                    &[
                        ("net", s(&f.net)),
                        ("terminal", pad(&f.terminal)),
                        ("upstream_net", s(&f.upstream_net)),
                        ("upstream_terminal", pad(&f.upstream_terminal)),
                        ("provenance", s(&f.provenance)),
                    ],
                ))
            })
            .collect();
        document.push(("power_feeds", j_list(feeds)));
    }
    let mut tracks: Vec<_> = board.tracks.iter().map(track_identity).collect();
    tracks.sort();
    document.push((
        "tracks",
        j_list(
            tracks
                .into_iter()
                .map(|(net, layer, a, b, width)| {
                    j_list(vec![
                        j_str(&net),
                        j_str(&layer),
                        j_list(vec![j_int(a.0), j_int(a.1)]),
                        j_list(vec![j_int(b.0), j_int(b.1)]),
                        j_int(width),
                    ])
                })
                .collect(),
        ),
    ));
    let mut vias: Vec<ViaIdentity> = board.vias.iter().map(via_identity).collect();
    vias.sort();
    document.push((
        "vias",
        j_list(
            vias.into_iter()
                .map(|v| {
                    j_list(vec![
                        j_str(&v.0),
                        j_int(v.1),
                        j_int(v.2),
                        j_str(&v.3),
                        j_str(&v.4),
                        j_int(v.5),
                        j_int(v.6),
                        v.7.as_deref().map_or(Json::Null, j_str),
                        j_str(&v.8),
                    ])
                })
                .collect(),
        ),
    ));
    if !board.polygons.is_empty() {
        document.push(("polygons", reprs(&board.polygons, |p| p.id.clone(), copper_polygon)));
    }
    let mut routing: Vec<&NetRoutingRule> = board.net_routing_rules.iter().collect();
    routing.sort_by(|a, b| a.net.cmp(&b.net));
    document.push(("routing_rules", j_list(routing.into_iter().map(routing_rule_document).collect())));
    if !board.decoupling_links.is_empty() {
        let links = board
            .decoupling_links
            .iter()
            .map(|l| {
                Json::Str(dc(
                    "DecouplingLink",
                    &[
                        ("capacitor", pad(&l.capacitor)),
                        ("target", pad(&l.target)),
                        ("return_pad", pad(&l.return_pad)),
                        ("net", s(&l.net)),
                        ("return_net", s(&l.return_net)),
                    ],
                ))
            })
            .collect();
        document.push(("decoupling_links", j_list(links)));
    }
    if !board.match_groups.is_empty() {
        let mut groups: Vec<&NetMatchGroup> = board.match_groups.iter().collect();
        groups.sort_by(|a, b| a.id.cmp(&b.id));
        document.push((
            "match_groups",
            j_list(
                groups
                    .into_iter()
                    .map(|g| {
                        j_list(vec![
                            j_str(&g.id),
                            j_list(g.nets.iter().map(|n| j_str(n)).collect()),
                            j_int(g.max_skew_nm),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    if !board.component_hole_clearances.is_empty() {
        let mut clearances: Vec<&ComponentHoleClearance> = board.component_hole_clearances.iter().collect();
        clearances.sort_by(|a, b| a.reference.cmp(&b.reference));
        document.push((
            "component_hole_clearances",
            j_list(
                clearances
                    .into_iter()
                    .map(|c| j_list(vec![j_str(&c.reference), j_int(c.clearance_nm), j_str(&c.reason)]))
                    .collect(),
            ),
        ));
    }
    if board.via_in_pad_rules.iter().any(|r| r.rows.is_some()) {
        let mut arrays: Vec<&PadViaInPadRule> = board.via_in_pad_rules.iter().filter(|r| r.rows.is_some()).collect();
        arrays.sort_by(|a, b| a.pad.cmp(&b.pad));
        document.push((
            "via_in_pad_arrays",
            j_list(
                arrays
                    .into_iter()
                    .map(|r| {
                        Json::Str(dc(
                            "PadViaInPadRule",
                            &[
                                ("pad", pad(&r.pad)),
                                ("process", s("filled-capped")),
                                ("rows", opt_int(r.rows)),
                                ("columns", opt_int(r.columns)),
                                ("pitch_nm", opt_int(r.pitch_nm)),
                            ],
                        ))
                    })
                    .collect(),
            ),
        ));
    }
    document.push(("regions", reprs(&board.regions, |r| r.name.clone(), region)));
    document.push(("keepouts", reprs(&board.keepouts, |k| k.name.clone(), placement_keepout)));
    document.push(("placement_rules", reprs(&board.placement_rules, |r| r.reference.clone(), placement_rule)));
    document
        .push(("relative_rules", j_list(board.relative_rules.iter().map(|r| Json::Str(relative_rule(r))).collect())));
    document.push(("placement_groups", reprs(&board.placement_groups, |g| g.name.clone(), placement_group)));
    document.push(("rigid_clusters", reprs(&board.rigid_clusters, |c| c.name.clone(), rigid_cluster)));
    document.push(("hard_macros", reprs(&board.hard_macros, |m| m.cluster.clone(), hard_macro)));
    if board.hard_macros.iter().any(|m| !m.zones.is_empty() || !m.plane_returns.is_empty() || !m.polygons.is_empty()) {
        let mut macros: Vec<&PhysicalHardMacro> = board.hard_macros.iter().collect();
        macros.sort_by(|a, b| a.cluster.cmp(&b.cluster));
        let rows = macros
            .into_iter()
            .map(|m| {
                let strings = |items: Vec<String>| j_list(items.into_iter().map(Json::Str).collect());
                let mut row = vec![
                    j_str(&m.cluster),
                    strings(m.zones.iter().map(zone).collect()),
                    strings(m.plane_returns.iter().map(plane_return).collect()),
                ];
                if !m.polygons.is_empty() {
                    row.push(strings(m.polygons.iter().map(copper_polygon).collect()));
                }
                j_list(row)
            })
            .collect();
        document.push(("hard_macro_local_copper", j_list(rows)));
    }
    document.push(("materialized_macros", j_list(board.materialized_macros.iter().map(|m| j_str(m)).collect())));
    document.push(("zones", reprs(&board.zones, |z| z.id.clone(), zone)));
    if board.zones.iter().any(|z| z.reserve_routing) {
        let mut ids: Vec<&String> = board.zones.iter().filter(|z| z.reserve_routing).map(|z| &z.id).collect();
        ids.sort();
        document.push(("zone_routing_reservations", j_list(ids.into_iter().map(|i| j_str(i)).collect())));
    }
    if board.zones.iter().any(|z| z.allow_same_net_hard_macro_overlap) {
        let mut ids: Vec<&String> =
            board.zones.iter().filter(|z| z.allow_same_net_hard_macro_overlap).map(|z| &z.id).collect();
        ids.sort();
        document.push(("same_net_hard_macro_overlaps", j_list(ids.into_iter().map(|i| j_str(i)).collect())));
    }
    document.push(("copper_keepouts", reprs(&board.copper_keepouts, |k| k.id.clone(), copper_keepout)));
    document.push(("zone_fills", j_list(Vec::new())));
    let mut metadata: Vec<(&String, &String)> = board.metadata.iter().collect();
    metadata.sort();
    document.push(("metadata", j_list(metadata.into_iter().map(|(k, v)| j_list(vec![j_str(k), j_str(v)])).collect())));
    Json::Object(document.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// `_digest`: SHA-256 of the compact, key-sorted JSON.
pub fn json_digest(document: &Json) -> String {
    let text = Dumps::new().sort_keys(true).separators(",", ":").dumps(document);
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `physical_board_digest`.
pub fn physical_board_digest(board: &PhysicalBoard) -> String {
    json_digest(&board_document(board))
}

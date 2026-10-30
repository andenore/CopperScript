//! `pcbir.editor.definitions`: local authoring sessions for macro and
//! board_profile documents. Coordinates are local and a viewport is never
//! a manufacturing outline; consumer qualification stays pending.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use copper_json::{Object, Value};
use copper_lang::footprints::FootprintResolver;
use copper_lang::packages::{PackageResolver, find_manifest};
use copper_lang::paths;
use copper_lang::syntax::{Declaration, Document, DocumentKind, MechanicalDecl, MechanicalItem, MechanicalItemDecl};
use copper_pcb::board::{
    CopperPolygon, PadReference, PhysicalBoard, PhysicalNet, Placement, Stackup, TrackSegment, Via,
};
use copper_physical::geometry::{RoundedConvexShape, shapes_clear};
use copper_physical::{BoardOutline, CopperLayer, DesignRules, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::definition_source::{definition_features, definition_patch, dumps};
use crate::difflib::{splitlines_keepends, unified_diff};
use crate::source::SourceSnapshot;
use crate::{Error, Result, object, sha256_hex, value};

fn source_edit<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

/// A board with only these fields; every other field has its default.
fn bare_board(name: &str, outline: BoardOutline) -> PhysicalBoard {
    PhysicalBoard {
        name: name.to_owned(),
        outline,
        footprints: IndexMap::new(),
        placements: Vec::new(),
        nets: Vec::new(),
        stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
        rules: DesignRules::default(),
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

/// `_viewport`: the points' bounding box with a 5 mm margin.
fn viewport(points: &[[i64; 2]]) -> Result<BoardOutline> {
    let points: Vec<[i64; 2]> = if points.is_empty() { vec![[0, 0]] } else { points.to_vec() };
    let margin = 5_000_000;
    let (x0, x1) = (points.iter().map(|p| p[0]).min().expect("x"), points.iter().map(|p| p[0]).max().expect("x"));
    let (y0, y1) = (points.iter().map(|p| p[1]).min().expect("y"), points.iter().map(|p| p[1]).max().expect("y"));
    let ring = vec![
        Point::new(x0 - margin, y0 - margin),
        Point::new(x1 + margin, y0 - margin),
        Point::new(x1 + margin, y1 + margin),
        Point::new(x0 - margin, y1 + margin),
    ];
    BoardOutline::new(ring, Vec::new(), None, None).map_err(Error::Value)
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(n) => i64::try_from(n).unwrap_or(0),
        _ => 0,
    }
}

fn pair(value: &Value) -> [i64; 2] {
    match value.as_array() {
        Some(items) if items.len() == 2 => [int(&items[0]), int(&items[1])],
        _ => [0, 0],
    }
}

fn pair_value(point: [i64; 2]) -> Value {
    Value::Array(vec![Value::from(point[0]), Value::from(point[1])])
}

fn rows<'a>(data: &'a Value, key: &str) -> &'a [Value] {
    data.get(key).and_then(Value::as_array).unwrap_or(&[])
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn layer(name: &str) -> Result<CopperLayer> {
    CopperLayer::parse(name)
        .ok_or_else(|| Error::Value(format!("{} is not a valid CopperLayer", dumps(name).replace('"', "'"))))
}

/// One track segment: net, layer, width and its two board points.
type Segment = (String, String, i64, [i64; 2], [i64; 2]);

/// A profile catalog entry: kind, shape and its literal parameters.
type Template<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

struct MacroBoard {
    board: PhysicalBoard,
}

impl MacroBoard {
    /// `endpoint`: a literal point or a member pad's board position.
    fn endpoint(&self, value: &Value) -> Result<[i64; 2]> {
        if value.as_array().is_some() {
            return Ok(pair(value));
        }
        let pad = value.get("pad").and_then(Value::as_array).map(<[Value]>::to_vec).unwrap_or_default();
        let (reference, number) =
            (pad.first().and_then(Value::as_str).unwrap_or(""), pad.get(1).and_then(Value::as_str).unwrap_or(""));
        let Some(pose) = self.board.placements.iter().find(|p| p.reference == reference) else {
            return Err(Error::Uncaught("StopIteration".to_owned()));
        };
        let Some(land) = self.board.footprints[&pose.footprint].pads.iter().find(|p| p.number == number) else {
            return Err(Error::Uncaught("StopIteration".to_owned()));
        };
        let point = copper_pcb::transform::transformed_local_point(pose, land.position);
        Ok([point.x_nm, point.y_nm])
    }
}

/// `_macro_scene`: the authored macro on a viewport board.
fn macro_scene(
    document: &Document,
    resolver: &PackageResolver,
    revision: &str,
) -> Result<(Object, IndexMap<String, String>)> {
    let (data, inputs) =
        copper_lang::macros::lower_macro(document, resolver, true).map_err(|failure| match failure {
            copper_lang::macros::MacroError::Compile(error) => Error::Copper(error),
            copper_lang::macros::MacroError::Value(message) => Error::Value(message),
        })?;
    let mut footprints = FootprintResolver::new(&resolver.source_directory, &[], false, true, true);
    let mut board = bare_board(&document.name, viewport(&[])?);
    for row in rows(&data, "members") {
        let footprint = text(row, "footprint");
        let resolved = footprints.resolve(footprint).map_err(Error::Value)?.footprint;
        board.footprints.insert(footprint.to_owned(), resolved);
        let rotation = match row.get("rotation_degrees") {
            Some(Value::Str(s)) => Decimal::parse(s).map_err(|e| Error::Value(e.to_string()))?,
            Some(other) => {
                Decimal::parse(&copper_json::Dumps::new().dumps(other)).map_err(|e| Error::Value(e.to_string()))?
            }
            None => Decimal::zero(),
        };
        let center = pair(row.get("center_nm").unwrap_or(&Value::Null));
        board.placements.push(Placement::new(
            text(row, "reference"),
            footprint,
            Point::new(center[0], center[1]),
            &rotation,
        )?);
    }
    let pad_nets: Vec<(String, String, String)> = rows(&data, "pad_nets")
        .iter()
        .map(|r| {
            let item = |i: usize| r.as_array().and_then(|a| a.get(i)).and_then(Value::as_str).unwrap_or("").to_owned();
            (item(0), item(1), item(2))
        })
        .collect();
    let names: std::collections::BTreeSet<&str> = pad_nets.iter().map(|(_, _, n)| n.as_str()).collect();
    board.nets = names
        .iter()
        .map(|net| PhysicalNet {
            name: (*net).to_owned(),
            pads: pad_nets.iter().filter(|(_, _, n)| n == net).map(|(r, p, _)| PadReference::new(r, p)).collect(),
        })
        .collect();
    let host = MacroBoard { board };
    let mut tracks: Vec<Value> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();
    for track in rows(&data, "tracks") {
        let points = rows(track, "points");
        for window in points.windows(2) {
            let (start, end) = (host.endpoint(&window[0])?, host.endpoint(&window[1])?);
            let (net, layer, width) =
                (text(track, "net"), text(track, "layer"), int(track.get("width_nm").unwrap_or(&Value::Null)));
            segments.push((net.to_owned(), layer.to_owned(), width, start, end));
            tracks.push(object(vec![
                ("net", Value::from(net)),
                ("layer", Value::from(layer)),
                ("width_nm", Value::from(width)),
                ("start", pair_value(start)),
                ("end", pair_value(end)),
            ]));
        }
    }
    let mut points: Vec<[i64; 2]> =
        rows(&data, "members").iter().map(|r| pair(r.get("center_nm").unwrap_or(&Value::Null))).collect();
    points.extend(segments.iter().flat_map(|s| [s.3, s.4]));
    let mut board = host.board;
    board.outline = viewport(&points)?;
    copper_pcb::physicalize::revalidate(&mut board)?;
    let host = MacroBoard { board };
    validate_ports(&host, &data, &segments)?;
    let board = host.board.clone();
    let mut scene = crate::scene::board_scene(&board, revision, 0, &Default::default(), None)?;
    let empty = || Value::Array(Vec::new());
    scene.insert(
        "outline".to_owned(),
        object(vec![("vertices", empty()), ("path", Value::Null), ("circle", Value::Null), ("cutouts", empty())]),
    );
    scene.insert("macro_definition".to_owned(), data.clone());
    let mut geometry = Vec::new();
    let kinds = [
        ("track", "tracks"),
        ("via", "vias"),
        ("port", "ports"),
        ("polygon", "polygons"),
        ("zone", "zones"),
        ("keepout", "keepouts"),
        ("protected_region", "protected_regions"),
    ];
    for (kind, key) in kinds {
        let names: Vec<&str> = document
            .declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::MacroItem(m) if m.kind == kind => Some(m.name.as_str()),
                _ => None,
            })
            .collect();
        for (name, row) in names.iter().zip(rows(&data, key)) {
            let layers = match (row.get("layers"), row.get("layer")) {
                (Some(layers), _) => layers.clone(),
                (None, Some(layer)) => Value::Array(vec![layer.clone()]),
                (None, None) => empty(),
            };
            let mut item = Object::new();
            item.insert("category".to_owned(), Value::from("macro"));
            item.insert("kind".to_owned(), Value::from(kind));
            item.insert("name".to_owned(), Value::from(*name));
            item.insert("net".to_owned(), row.get("net").cloned().unwrap_or(Value::Null));
            item.insert("layers".to_owned(), layers);
            if let Some(vertices) = row.get("vertices") {
                item.insert("vertices".to_owned(), vertices.clone());
            }
            if let Some(points) = row.get("points") {
                let points: Vec<Value> = points
                    .as_array()
                    .unwrap_or(&[])
                    .iter()
                    .map(|p| host.endpoint(p).map(pair_value))
                    .collect::<Result<_>>()?;
                item.insert("points".to_owned(), Value::Array(points));
                item.insert("width_nm".to_owned(), row.get("width_nm").cloned().unwrap_or(Value::Null));
            }
            if let Some(point) = row.get("point") {
                item.insert("position".to_owned(), pair_value(host.endpoint(point)?));
            }
            if let Some(position) = row.get("position_nm") {
                item.insert("position".to_owned(), position.clone());
                item.insert("size_nm".to_owned(), row.get("size_nm").cloned().unwrap_or(Value::Null));
            }
            geometry.push(Value::Object(item));
        }
    }
    scene.insert("physical_geometry".to_owned(), Value::Array(geometry));
    let vias: Vec<Value> = rows(&data, "vias")
        .iter()
        .map(|v| {
            let mut via = v.as_object().cloned().unwrap_or_default();
            via.insert("position".to_owned(), v.get("position_nm").cloned().unwrap_or(Value::Null));
            Value::Object(via)
        })
        .collect();
    scene.insert(
        "routed_overlay".to_owned(),
        object(vec![
            ("stale", Value::from(false)),
            ("connectivity_credit", Value::from(false)),
            ("notice", Value::from("Authored macro copper; consumer validation and native DRC pending")),
            (
                "native",
                object(vec![
                    ("layers", data.get("required_layers").cloned().unwrap_or_else(empty)),
                    ("fills", empty()),
                    ("tracks", Value::Array(tracks)),
                    ("vias", Value::Array(vias)),
                ]),
            ),
            ("evidence", object(vec![("remaining", empty())])),
        ]),
    );
    let keepouts: Vec<Value> = rows(&data, "keepouts")
        .iter()
        .chain(rows(&data, "protected_regions"))
        .map(|r| {
            object(vec![
                ("name", r.get("id").cloned().unwrap_or(Value::Null)),
                ("layers", r.get("layers").cloned().unwrap_or(Value::Null)),
                ("vertices", r.get("vertices").cloned().unwrap_or(Value::Null)),
                ("holes", empty()),
            ])
        })
        .collect();
    scene.insert("copper_keepouts".to_owned(), Value::Array(keepouts));
    if let Some(Value::Object(warnings)) = scene.get_mut("warnings") {
        warnings.insert(
            "qualification_pending".to_owned(),
            Value::from("Local port copper continuity checked; macro binding, host stackup, plane-return proof and native DRC require consumer validation."),
        );
    }
    Ok((scene, inputs))
}

/// `_validate_ports`: each port touches owner copper connected to exactly its private pads.
fn validate_ports(host: &MacroBoard, data: &Value, segments: &[Segment]) -> Result<()> {
    let ports = rows(data, "ports");
    if ports.is_empty() {
        return Ok(());
    }
    let required: Vec<&str> = rows(data, "required_layers").iter().filter_map(Value::as_str).collect();
    let mut order = vec![CopperLayer::Front];
    for i in 1..31 {
        let name = format!("In{i}.Cu");
        if required.contains(&name.as_str()) {
            order.push(layer(&name)?);
        }
    }
    order.push(CopperLayer::Back);
    let layers: Vec<CopperLayer> = required.iter().map(|l| layer(l)).collect::<Result<_>>()?;
    let span: Vec<CopperLayer> = order
        .into_iter()
        .filter(|l| layers.contains(l) || matches!(l, CopperLayer::Front | CopperLayer::Back))
        .collect();
    let tracks: Vec<TrackSegment> = segments
        .iter()
        .map(|(net, l, width, start, end)| {
            Ok(TrackSegment {
                net: net.clone(),
                layer: layer(l)?,
                start: Point::new(start[0], start[1]),
                end: Point::new(end[0], end[1]),
                width_nm: *width,
            })
        })
        .collect::<Result<_>>()?;
    let vias: Vec<Via> = rows(data, "vias")
        .iter()
        .map(|r| {
            let position = pair(r.get("position_nm").unwrap_or(&Value::Null));
            Ok(Via {
                net: text(r, "net").to_owned(),
                position: Point::new(position[0], position[1]),
                size_nm: int(r.get("size_nm").unwrap_or(&Value::Null)),
                drill_nm: int(r.get("drill_nm").unwrap_or(&Value::Null)),
                from_layer: layer(text(r, "from_layer"))?,
                to_layer: layer(text(r, "to_layer"))?,
                technology: r.get("technology").and_then(Value::as_str).map(str::to_owned),
                finish: text(r, "finish").to_owned(),
            })
        })
        .collect::<Result<_>>()?;
    let polygons: Vec<CopperPolygon> = rows(data, "polygons")
        .iter()
        .map(|r| {
            Ok(CopperPolygon {
                id: text(r, "id").to_owned(),
                net: text(r, "net").to_owned(),
                layer: layer(text(r, "layer"))?,
                vertices: rows(r, "vertices")
                    .iter()
                    .map(|p| {
                        let p = pair(p);
                        Point::new(p[0], p[1])
                    })
                    .collect(),
            })
        })
        .collect::<Result<_>>()?;
    let mut owner = host.board.clone();
    owner.tracks = tracks.clone();
    owner.vias = vias.clone();
    owner.stackup = Stackup::new(span.clone());
    let candidate = copper_drc::connectivity::Candidate {
        only_nets: None,
        exclude_internal_connections: false,
        tracks: None,
        vias: None,
        polygons: Some(&polygons),
    };
    let graph = copper_drc::connectivity::explicit_copper_connectivity(&owner, candidate)?;
    for port in ports {
        let position = host.endpoint(port.get("point").unwrap_or(&Value::Null))?;
        let position = Point::new(position[0], position[1]);
        let port_layer = layer(text(port, "layer"))?;
        let net = text(port, "net");
        let probe = RoundedConvexShape::new(vec![position], 0).map_err(Error::Value)?;
        let mut roots: Vec<&String> = Vec::new();
        for (i, track) in tracks.iter().enumerate() {
            if track.net == net && track.layer == port_layer {
                let shape =
                    RoundedConvexShape::new(vec![track.start, track.end], track.width_nm / 2).map_err(Error::Value)?;
                if !shapes_clear(&probe, &shape, 1) {
                    let root = &graph.roots[&format!("track:{i}")];
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                }
            }
        }
        for (i, via) in vias.iter().enumerate() {
            let index = |l: CopperLayer| span.iter().position(|s| *s == l);
            let (Some(a), Some(b)) = (index(via.from_layer), index(via.to_layer)) else {
                return value(format!("{} is not in tuple", via.from_layer.value()));
            };
            let (a, b) = (a.min(b), a.max(b));
            if via.net == net && via.position == position && span[a..=b].contains(&port_layer) {
                let root = &graph.roots[&format!("via:{i}")];
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        if roots.is_empty() {
            return source_edit("macro port has no owner copper contact");
        }
        let pads_connected = rows(port, "pads").iter().all(|p| {
            let item = |i: usize| p.as_array().and_then(|a| a.get(i)).and_then(Value::as_str).unwrap_or("");
            match graph.pad_nodes.get(&PadReference::new(item(0), item(1))) {
                Some(nodes) if !nodes.is_empty() => nodes.iter().all(|n| roots.contains(&&graph.roots[n])),
                _ => false,
            }
        });
        if roots.len() != 1 || !pads_connected {
            return source_edit("macro port is not connected to its declared private pads");
        }
    }
    Ok(())
}

/// `_profile_scene`: a board_profile's mechanical items on a bare board.
fn profile_scene(
    document: &Document,
    resolver: &mut PackageResolver,
    revision: &str,
) -> Result<(Object, IndexMap<String, String>)> {
    let recorded: Arc<Mutex<IndexMap<String, String>>> = Arc::default();
    let sink = recorded.clone();
    resolver.source_reader = Some(Arc::new(move |path: &Path| {
        let raw = std::fs::read(path)?;
        sink.lock().unwrap_or_else(|p| p.into_inner()).insert(paths::display(&paths::resolve(path)), sha256_hex(&raw));
        let body = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw);
        String::from_utf8(body.to_vec()).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }));
    let imported = copper_lang::compiler::document_imports(document, resolver)?;
    let declared: Vec<&MechanicalItem> = document
        .declarations
        .iter()
        .filter_map(|d| match d {
            Declaration::MechanicalItem(item) => Some(item),
            _ => None,
        })
        .collect();
    let (items, sources, instances) = copper_lang::mechanical::expand_mechanical_items(&declared, &imported.profiles)?;
    let roles = ["connector", "attach", "overhang", "component_height", "assembly_access"];
    let mut items: Vec<MechanicalItemDecl> = items
        .into_iter()
        .map(|mut item| {
            if roles.contains(&item.kind.as_str()) && !item.parameters.contains_key("component") {
                let name = copper_lang::syntax::Scalar::Str(item.name.clone());
                item.parameters.insert("component".to_owned(), copper_lang::syntax::Value::Scalar(name));
            }
            item
        })
        .collect();
    let has_outline = items.iter().any(|i| i.kind == "outline");
    if !has_outline {
        // Python's viewport is 2 km square, beyond this implementation's 2^40 nm
        // lengths: lower against a 2^40 nm stand-in, then substitute it.
        let viewport = copper_lang::parse(
            "board_profile Viewport { outline rectangle { width = 1099511.627776 mm; height = 1099511.627776 mm; origin = (-549755.813888 mm, -549755.813888 mm); } }",
            "<memory>",
        )?;
        if let Some(Declaration::MechanicalItem(MechanicalItem::Feature(outline))) =
            viewport.declarations.into_iter().next()
        {
            items.insert(0, outline);
        }
    }
    let own: &[MechanicalItemDecl] = if has_outline { &items } else { &items[1..] };
    let own = own.to_vec();
    let wrapped = Document {
        kind: DocumentKind::Board,
        declarations: vec![Declaration::Mechanical(MechanicalDecl {
            location: document.location.clone(),
            items: items.into_iter().map(MechanicalItem::Feature).collect(),
        })],
        ..document.clone()
    };
    let lowered = copper_lang::mechanical::lower_mechanical(&wrapped, &IndexMap::new());
    let mut mechanical = match lowered {
        Ok(mechanical) => mechanical.expect("mechanical block"),
        Err(error) if !has_outline => {
            return Err(Error::NotImplemented(format!(
                "a board_profile without an outline whose features fail against the 2^40 nm viewport ({error})"
            )));
        }
        Err(error) => return Err(error.into()),
    };
    if !has_outline {
        let (low, high) = (-1_000_000_000_000, 1_000_000_000_000);
        let ring = vec![Point::new(low, low), Point::new(high, low), Point::new(high, high), Point::new(low, high)];
        mechanical.outline = BoardOutline::new(ring, Vec::new(), None, None).map_err(Error::Value)?;
    }
    let mut board = bare_board(&document.name, mechanical.outline.clone());
    board.mechanical_holes = mechanical.holes.clone();
    board.mechanical_slots = mechanical.slots.clone();
    board.keepouts = mechanical.keepouts.clone();
    board.copper_keepouts = mechanical.copper_keepouts.clone();
    board.datums = mechanical.datums.clone();
    board.boundary_edges = mechanical.boundary_edges.clone();
    board.mechanical_references = mechanical.references.clone();
    board.assembly_envelopes = mechanical.assembly_envelopes.clone();
    copper_pcb::physicalize::revalidate(&mut board)?;
    let mut scene = crate::scene::board_scene(&board, revision, 0, &Default::default(), None)?;
    mechanical.sources = sources.clone();
    mechanical.profiles = instances;
    scene.insert("mechanical_provenance".to_owned(), copper_lang::mechanical::mechanical_provenance(&mechanical));
    let mut inherited = Vec::new();
    for (item, source) in own.iter().zip(&sources) {
        let Some(profile) = &source.profile else { continue };
        let owner = PathBuf::from(&*source.location.filename);
        let raw = crate::dependencies::read_source(&owner)?;
        let snapshot = SourceSnapshot::new(raw, &paths::display(&owner))?;
        let short = item.name.rsplit('/').next().unwrap_or("");
        let original = definition_features(&snapshot)?.into_iter().find(|f| {
            f.get("kind").and_then(Value::as_str) == Some(item.kind.as_str())
                && f.get("name").and_then(Value::as_str) == Some(short)
        });
        if let Some(Value::Object(mut feature)) = original {
            feature.insert("name".to_owned(), Value::from(item.name.as_str()));
            feature.insert("editable".to_owned(), Value::from(false));
            feature.insert("definition_path".to_owned(), Value::from(paths::display(&paths::resolve(&owner))));
            feature.insert("profile".to_owned(), Value::from(profile.as_str()));
            inherited.push(Value::Object(feature));
        }
    }
    scene.insert("inherited_features".to_owned(), Value::Array(inherited));
    let roles: Vec<Value> = mechanical
        .connectors
        .iter()
        .map(|c| {
            object(vec![
                ("name", Value::from(c.role.as_str())),
                ("position", pair_value([c.position.x_nm, c.position.y_nm])),
                ("rotation", Value::from(c.rotation.to_sci_string())),
                ("side", Value::from(c.side.value())),
                ("anchor_pad", Value::from(c.anchor_pad.as_str())),
            ])
        })
        .collect();
    let geometry: Vec<Value> = roles
        .iter()
        .map(|role| {
            let mut entry = Object::new();
            entry.insert("category".to_owned(), Value::from("profile"));
            entry.insert("kind".to_owned(), Value::from("connector"));
            entry.extend(role.as_object().cloned().unwrap_or_default());
            Value::Object(entry)
        })
        .collect();
    scene.insert("connector_roles".to_owned(), Value::Array(roles.clone()));
    scene.insert("physical_geometry".to_owned(), Value::Array(geometry));
    if !has_outline {
        let empty = || Value::Array(Vec::new());
        scene.insert(
            "outline".to_owned(),
            object(vec![("vertices", empty()), ("path", Value::Null), ("circle", Value::Null), ("cutouts", empty())]),
        );
        let mut points: Vec<[i64; 2]> = rows(&Value::Object(scene.clone()), "holes")
            .iter()
            .map(|h| pair(h.get("position").unwrap_or(&Value::Null)))
            .collect();
        points.extend(roles.iter().map(|r| pair(r.get("position").unwrap_or(&Value::Null))));
        let vertices = viewport(&points)?.vertices;
        let bounds = [vertices[0].x_nm, vertices[0].y_nm, vertices[2].x_nm, vertices[2].y_nm];
        scene.insert("bounds".to_owned(), Value::Array(bounds.iter().map(|v| Value::from(*v)).collect()));
    }
    if let Some(Value::Object(warnings)) = scene.get_mut("warnings") {
        warnings.insert(
            "qualification_pending".to_owned(),
            Value::from("Profile roles and fragment boundaries require consumer validation."),
        );
    }
    let mut inputs = recorded.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for reference in &mechanical.references {
        let path = PathBuf::from(&reference.asset_path);
        if !reference.asset_path.is_empty() && path.is_file() {
            let raw = crate::dependencies::read_source(&path)?;
            inputs.insert(paths::display(&paths::resolve(&path)), sha256_hex(&raw));
        }
    }
    Ok((scene, inputs))
}

fn texts(entries: &[(&str, &str)]) -> Value {
    Value::Object(entries.iter().map(|(k, v)| ((*k).to_owned(), Value::from(*v))).collect())
}

/// `feature_catalog(scene)`: new-feature templates for a definition document.
fn definition_catalog(scene: &Object) -> Vec<Value> {
    if scene.get("document_kind").and_then(Value::as_str) == Some("board_profile") {
        let polygon = "[(0 mm, 0 mm), (4 mm, 0 mm), (4 mm, 4 mm), (0 mm, 4 mm)]";
        let rows: Vec<Template> = vec![
            ("hole", "", vec![("position", "(0 mm, 0 mm)"), ("diameter", "2 mm")]),
            (
                "connector",
                "",
                vec![("position", "(0 mm, 0 mm)"), ("anchor_pad", "\"1\""), ("rotation", "0"), ("side", "front")],
            ),
            ("outline", "rectangle", vec![("width", "50 mm"), ("height", "30 mm")]),
            ("outline", "circle", vec![("diameter", "50 mm"), ("center", "(25 mm, 25 mm)")]),
            ("outline", "rounded_rectangle", vec![("width", "50 mm"), ("height", "30 mm"), ("corner_radius", "3 mm")]),
            ("outline", "polygon", vec![("vertices", polygon)]),
            ("outline", "path", vec![("maximum_chord_error", "0.01 mm")]),
            ("boundary", "line", vec![("start", "(0 mm, 0 mm)"), ("end", "(4 mm, 0 mm)")]),
            ("boundary", "arc", vec![("start", "(0 mm, 0 mm)"), ("mid", "(2 mm, 2 mm)"), ("end", "(4 mm, 0 mm)")]),
            ("slot", "", vec![("start", "(2 mm, 2 mm)"), ("end", "(4 mm, 2 mm)"), ("width", "1 mm")]),
            ("cutout", "polygon", vec![("vertices", polygon)]),
            (
                "keepout",
                "rectangle",
                vec![("width", "4 mm"), ("height", "4 mm"), ("origin", "(0 mm, 0 mm)"), ("side", "front")],
            ),
            ("keepout", "polygon", vec![("vertices", polygon), ("side", "front")]),
            (
                "copper_keepout",
                "rectangle",
                vec![("width", "4 mm"), ("height", "4 mm"), ("origin", "(0 mm, 0 mm)"), ("layers", "\"F.Cu\"")],
            ),
            ("copper_keepout", "polygon", vec![("vertices", polygon), ("layers", "\"F.Cu\"")]),
            ("rules", "", vec![("minimum_clearance", "0.2 mm"), ("minimum_track_width", "0.2 mm")]),
            ("datum", "", vec![("position", "(0 mm, 0 mm)")]),
            ("edge", "", vec![("start", "(0 mm, 0 mm)"), ("end", "(4 mm, 0 mm)")]),
            (
                "attach",
                "",
                vec![("position", "(0 mm, 0 mm)"), ("anchor", "origin"), ("rotation", "0"), ("side", "front")],
            ),
            (
                "overhang",
                "",
                vec![
                    ("edge", "TOP"),
                    ("start", "1 mm"),
                    ("end", "3 mm"),
                    ("distance", "2 mm"),
                    ("reason", "\"Source evidence required\""),
                ],
            ),
            ("component_height", "", vec![("height", "3 mm")]),
            (
                "enclosure",
                "rectangle",
                vec![
                    ("origin", "(0 mm, 0 mm)"),
                    ("width", "4 mm"),
                    ("height", "4 mm"),
                    ("side", "front"),
                    ("maximum_height", "4 mm"),
                ],
            ),
            ("enclosure", "polygon", vec![("vertices", polygon), ("side", "front"), ("maximum_height", "4 mm")]),
            (
                "assembly_access",
                "rectangle",
                vec![
                    ("origin", "(0 mm, 0 mm)"),
                    ("width", "4 mm"),
                    ("height", "4 mm"),
                    ("side", "component"),
                    ("purpose", "\"Insertion clearance\""),
                ],
            ),
            (
                "assembly_access",
                "polygon",
                vec![("vertices", polygon), ("side", "component"), ("purpose", "\"Insertion clearance\"")],
            ),
            (
                "reference",
                "dxf",
                vec![
                    ("file", "\"assets/case.dxf\""),
                    ("sha256", "\"REPLACE_WITH_FILE_SHA256\""),
                    ("units", "mm"),
                    ("frame", "cartesian"),
                    ("position", "(0 mm, 0 mm)"),
                    ("rotation", "0"),
                    ("mirror_x", "false"),
                    ("side", "both"),
                    ("purpose", "\"Enclosure reference only\""),
                ],
            ),
        ];
        return rows
            .into_iter()
            .map(|(kind, shape, parameters)| {
                let name = if kind == "outline" || kind == "rules" { "" } else { "new_feature" };
                object(vec![
                    ("category", Value::from("profile")),
                    ("kind", Value::from(kind)),
                    ("name", Value::from(name)),
                    ("shape", Value::from(shape)),
                    ("parameters", texts(&parameters)),
                ])
            })
            .collect();
    }
    let features: &[Value] = scene.get("source_features").and_then(Value::as_array).unwrap_or(&[]);
    let named = |kind: &str| features.iter().find(|f| f.get("kind").and_then(Value::as_str) == Some(kind));
    let member = named("member").cloned().unwrap_or(Value::Null);
    let data = scene.get("macro_definition").cloned().unwrap_or(Value::Null);
    let first = rows(&data, "pad_nets").first().and_then(Value::as_array).map(|r| {
        let item = |i: usize| r.get(i).and_then(Value::as_str).unwrap_or("").to_owned();
        (item(0), item(1), item(2))
    });
    let (reference, pad, role) =
        first.unwrap_or_else(|| (text(&member, "name").to_owned(), "1".to_owned(), "NET_ROLE".to_owned()));
    let net = dumps(&role);
    let endpoint = format!("{}.{}", dumps(&reference), dumps(&pad));
    let layer = dumps(rows(&data, "required_layers").first().and_then(Value::as_str).unwrap_or(""));
    let vertices = "[(0 mm, 0 mm), (2 mm, 0 mm), (2 mm, 2 mm), (0 mm, 2 mm)]";
    let layers = format!("[{layer}]");
    let pads = format!("[{endpoint}]");
    let track = named("track").map_or_else(|| "\"track_name\"".to_owned(), |f| dumps(text(f, "name")));
    let keepout = named("keepout").map_or_else(|| "\"keepout_name\"".to_owned(), |f| dumps(text(f, "name")));
    let plane = named("plane_return").map_or("", |f| text(f, "name")).to_owned();
    let points = format!("[{endpoint}, (2 mm, 0 mm)]");
    let member_parameters = member.get("parameters").cloned().unwrap_or(Value::Object(Object::new()));
    let reference_text = dumps(&reference);
    let pad_text = dumps(&pad);
    let templates: Vec<(&str, Value)> = vec![
        ("net", texts(&[])),
        ("pad", texts(&[("member", &reference_text), ("pad", &pad_text), ("net", &net)])),
        ("isolated", texts(&[("member", &reference_text), ("pad", "\"PAD_NUMBER\"")])),
        ("member", member_parameters),
        ("track", texts(&[("net", &net), ("layer", &layer), ("width", "0.2 mm"), ("points", &points)])),
        (
            "via",
            texts(&[
                ("net", &net),
                ("at", "(0 mm, 0 mm)"),
                ("size", "0.6 mm"),
                ("drill", "0.3 mm"),
                ("from_layer", "\"F.Cu\""),
                ("to_layer", "\"B.Cu\""),
            ]),
        ),
        ("port", texts(&[("net", &net), ("layer", &layer), ("point", &endpoint), ("pads", &pads)])),
        ("polygon", texts(&[("net", &net), ("layer", &layer), ("vertices", vertices)])),
        (
            "zone",
            texts(&[
                ("net", &net),
                ("layers", &layers),
                ("vertices", vertices),
                ("priority", "0"),
                ("clearance", "0.2 mm"),
                ("minimum_width", "0.2 mm"),
                ("pad_connection", "solid"),
            ]),
        ),
        ("keepout", texts(&[("layers", &layers), ("vertices", vertices)])),
        ("protected_region", texts(&[("layers", &layers), ("vertices", vertices)])),
        ("plane_return", texts(&[("net", &net), ("layers", &layers), ("pads", &pads)])),
        ("contact", texts(&[("pad", &endpoint), ("at", "(0 mm, 0 mm)")])),
        (
            "width_contract",
            texts(&[
                ("track", &track),
                ("minimum_width", "0.1 mm"),
                ("purpose", "pin_entry"),
                ("evidence", "\"Unresolved: supply source evidence\""),
            ]),
        ),
        ("edge_opening", texts(&[("keepout", &keepout), ("edge_indices", "[0]")])),
    ];
    templates
        .into_iter()
        .map(|(kind, parameters)| {
            object(vec![
                ("category", Value::from("macro")),
                ("kind", Value::from(kind)),
                ("name", Value::from("new_feature")),
                ("parameters", parameters),
                ("parent", Value::from(if kind == "contact" { plane.as_str() } else { "" })),
            ])
        })
        .collect()
}

/// `DefinitionReview`.
#[derive(Clone)]
struct DefinitionReview {
    before: SourceSnapshot,
    after: SourceSnapshot,
    board: Object,
    diff: String,
}

impl DefinitionReview {
    fn id(&self) -> String {
        sha256_hex(format!("{}{}", self.before.revision(), self.after.revision()).as_bytes())
    }
}

/// `DefinitionSession`: reviewed local authoring of one definition document.
pub struct DefinitionSession {
    source: PathBuf,
    dependency_source: PathBuf,
    readonly: bool,
    revision: i64,
    source_pending: Option<DefinitionReview>,
    undo: Vec<Vec<u8>>,
    redo: Vec<Vec<u8>>,
    inputs: Option<IndexMap<String, String>>,
    snapshot: SourceSnapshot,
    accepted: Object,
}

fn digest(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|raw| sha256_hex(&raw))
}

impl DefinitionSession {
    pub fn new(source: &Path, project: Option<&Path>) -> Result<Self> {
        let source = std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf());
        if source.ancestors().any(|p| p.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink())) {
            return source_edit("definition editing does not follow source symlinks");
        }
        let dependency_source = crate::dependencies::dependency_context(&source, project)?;
        let readonly = source.components().any(|p| p.as_os_str().to_string_lossy().to_lowercase() == ".copper-cache");
        let raw = crate::dependencies::read_source(&source)?;
        let snapshot = SourceSnapshot::new(raw, &paths::display(&source))?;
        let mut session = DefinitionSession {
            source,
            dependency_source,
            readonly,
            revision: 0,
            source_pending: None,
            undo: Vec::new(),
            redo: Vec::new(),
            inputs: None,
            snapshot,
            accepted: Object::new(),
        };
        session.accepted = session.validate(&session.snapshot.raw.clone())?;
        Ok(session)
    }

    fn read_source(&self) -> Result<Vec<u8>> {
        crate::dependencies::read_source(&self.source)
    }

    /// `build`: the definition's scene and every input it read.
    fn build(&self, raw: &[u8]) -> Result<(Object, IndexMap<String, String>)> {
        let snapshot = SourceSnapshot::new(raw.to_vec(), &paths::display(&self.source))?;
        let document = copper_lang::parse(&snapshot.text, &snapshot.filename)?;
        let mut resolver = PackageResolver::for_source(&self.dependency_source, true, true)?;
        if self.dependency_source != self.source {
            let modules: Vec<String> = resolver.manifest.requirements.keys().cloned().collect();
            for module in modules {
                let package = resolver.resolve(&module, &document.location)?;
                if self.source.starts_with(&package.directory) {
                    break;
                }
            }
        }
        let (mut scene, mut inputs) = match document.kind {
            DocumentKind::Macro => macro_scene(&document, &resolver, &snapshot.revision())?,
            DocumentKind::BoardProfile => profile_scene(&document, &mut resolver, &snapshot.revision())?,
            _ => return source_edit("definition editor requires macro or board_profile"),
        };
        if let Some(manifest) = find_manifest(&self.dependency_source) {
            let lock = manifest.parent().unwrap_or(Path::new("/")).join("copper.lock");
            for path in [manifest, lock] {
                if path.is_file() {
                    inputs.insert(paths::display(&paths::resolve(&path)), digest(&path).unwrap_or_default());
                }
            }
        }
        let writable = !self.readonly;
        let mut features = definition_features(&snapshot)?;
        for feature in &mut features {
            if let Value::Object(entry) = feature {
                entry.insert("editable".to_owned(), Value::from(writable));
            }
        }
        if let Some(Value::Array(inherited)) = scene.shift_remove("inherited_features") {
            features.extend(inherited);
        }
        scene.insert("document_kind".to_owned(), Value::from(document.kind.as_str()));
        scene.insert("source_features".to_owned(), Value::Array(features));
        scene.insert(
            "notice".to_owned(),
            Value::from(
                "Definition authoring in local coordinates. Consumer integration and qualification remain pending.",
            ),
        );
        if let Some(Value::Object(capabilities)) = scene.get_mut("capabilities") {
            for (key, enabled) in [
                ("auto_place", false),
                ("pose_preview", false),
                ("session_locks", false),
                ("source_save", writable),
                ("mechanical_edit", false),
                ("definition_edit", writable),
                ("physical_feature_edit", writable),
            ] {
                capabilities.insert(key.to_owned(), Value::from(enabled));
            }
        }
        scene.insert("source_writable".to_owned(), Value::from(writable));
        let catalog = definition_catalog(&scene);
        scene.insert("feature_catalog".to_owned(), Value::Array(catalog));
        Ok((scene, inputs))
    }

    /// `DefinitionWorkspace.check_inputs`.
    fn check_inputs(&self) -> Result<()> {
        for (path, expected) in self.inputs.iter().flatten() {
            let path = Path::new(path);
            if !path.is_file() || digest(path).as_ref() != Some(expected) {
                return source_edit("definition dependency changed; reload source");
            }
        }
        Ok(())
    }

    /// `DefinitionWorkspace.validate`.
    fn validate(&mut self, raw: &[u8]) -> Result<Object> {
        self.check_inputs()?;
        let (scene, inputs) = self.build(raw)?;
        match &self.inputs {
            None => self.inputs = Some(inputs),
            Some(current) if *current != inputs => {
                return source_edit("definition dependency inventory changed; reload source");
            }
            Some(_) => {}
        }
        self.check_inputs()?;
        Ok(scene)
    }

    /// `DefinitionWorkspace.save` through the board workspace's locked writer.
    fn save(&self, before: &SourceSnapshot, after: &SourceSnapshot) -> Result<()> {
        crate::transactions::save_reviewed(&self.source, &before.raw, &after.raw, &|| self.check_inputs())
    }

    pub fn scene(&mut self) -> Result<Object> {
        let mut result = self.accepted.clone();
        result.insert("revision".to_owned(), Value::from(self.revision));
        result.insert("can_undo".to_owned(), Value::from(false));
        result.insert("can_redo".to_owned(), Value::from(false));
        result.insert("can_source_undo".to_owned(), Value::from(!self.undo.is_empty()));
        result.insert("can_source_redo".to_owned(), Value::from(!self.redo.is_empty()));
        result.insert("source_stale".to_owned(), Value::from(self.read_source()? != self.snapshot.raw));
        let review = self
            .source_pending
            .as_ref()
            .map(|r| object(vec![("id", Value::from(r.id())), ("diff", Value::from(r.diff.as_str()))]));
        result.insert("source_review".to_owned(), review.unwrap_or(Value::Null));
        let preview = self.source_pending.as_ref().map(|r| Value::Object(r.board.clone()));
        result.insert("source_preview".to_owned(), preview.unwrap_or(Value::Null));
        Ok(result)
    }

    fn check(&self, revision: Option<&Value>) -> Result<()> {
        if !matches!(revision, Some(Value::Int(n)) if *n == self.revision.into()) {
            return source_edit("stale editor revision");
        }
        if self.read_source()? != self.snapshot.raw {
            return source_edit("source changed externally; reload source");
        }
        self.check_inputs()
    }

    fn accepted(&mut self) -> Result<Object> {
        Ok([("scene".to_owned(), Value::Object(self.scene()?)), ("accepted".to_owned(), Value::from(true))]
            .into_iter()
            .collect())
    }

    pub fn operation(&mut self, request: &Object) -> Result<Object> {
        let action = request.get("action").and_then(Value::as_str).unwrap_or("");
        let keys =
            |expected: &[&str]| request.len() == expected.len() && expected.iter().all(|k| request.contains_key(*k));
        if self.readonly && action != "reload_source" {
            return source_edit(
                "cached dependency definitions are read-only; open the owning library source or create an adaptation",
            );
        }
        if action == "reload_source" {
            if !keys(&["action", "revision"]) {
                return source_edit("unexpected reload fields");
            }
            if !matches!(request.get("revision"), Some(Value::Int(n)) if *n == self.revision.into()) {
                return source_edit("stale editor revision");
            }
            let previous = self.inputs.take();
            let raw = self.read_source()?;
            let scene = match self.validate(&raw) {
                Ok(scene) => scene,
                Err(error) => {
                    self.inputs = previous;
                    return Err(error);
                }
            };
            self.snapshot = SourceSnapshot::new(raw, &paths::display(&self.source))?;
            self.accepted = scene;
            self.source_pending = None;
            self.undo.clear();
            self.redo.clear();
            self.revision += 1;
            return self.accepted();
        }
        self.check(request.get("revision"))?;
        match action {
            "review_feature" | "review_features" => {
                let features: Vec<Value> = if action == "review_features" {
                    if !keys(&["action", "revision", "features"]) {
                        return source_edit("unexpected batch fields");
                    }
                    match &request["features"] {
                        Value::Array(items) if (1..=64).contains(&items.len()) => items.clone(),
                        _ => return source_edit("definition batch requires 1–64 feature edits"),
                    }
                } else {
                    let fields: Object = request
                        .iter()
                        .filter(|(k, _)| *k != "action" && *k != "revision")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    vec![Value::Object(fields)]
                };
                let allowed = ["category", "kind", "name", "parameters", "shape", "parent", "remove", "editable"];
                let mut current = self.snapshot.clone();
                for feature in &features {
                    let Value::Object(fields) = feature else { return source_edit("unexpected feature fields") };
                    if fields.keys().any(|k| !allowed.contains(&k.as_str())) {
                        return source_edit("unexpected feature fields");
                    }
                    if !["category", "kind", "name", "parameters"].iter().all(|k| fields.contains_key(*k)) {
                        return source_edit("feature requires category, kind, name and parameters");
                    }
                    let mut fields = fields.clone();
                    fields.shift_remove("editable");
                    let raw = definition_patch(&current, &fields)?.apply(&current)?;
                    current = SourceSnapshot::new(raw, &paths::display(&self.source))?;
                }
                if current.raw == self.snapshot.raw {
                    return source_edit("edit has no source changes");
                }
                let scene = self.validate(&current.raw)?;
                let diff = unified_diff(
                    &splitlines_keepends(&self.snapshot.text),
                    &splitlines_keepends(&current.text),
                    "current.copper",
                    "proposed.copper",
                );
                self.source_pending =
                    Some(DefinitionReview { before: self.snapshot.clone(), after: current, board: scene, diff });
            }
            "discard_source" => {
                if !keys(&["action", "revision"]) {
                    return source_edit("unexpected discard fields");
                }
                self.source_pending = None;
            }
            "save_source" => {
                if !keys(&["action", "revision", "review_id"]) {
                    return source_edit("unexpected save fields");
                }
                let review = match &self.source_pending {
                    Some(review) if request["review_id"].as_str() == Some(review.id().as_str()) => review.clone(),
                    _ => return source_edit("review changed"),
                };
                let scene = self.validate(&review.after.raw)?;
                if scene != review.board {
                    return source_edit("definition inputs changed after review");
                }
                self.save(&review.before, &review.after)?;
                self.undo.push(self.snapshot.raw.clone());
                if self.undo.len() > 64 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.snapshot = review.after;
                self.accepted = scene;
                self.source_pending = None;
            }
            "undo_source" | "redo_source" => {
                if !keys(&["action", "revision"]) {
                    return source_edit("unexpected history fields");
                }
                let undo = action == "undo_source";
                let Some(raw) = (if undo { self.undo.last() } else { self.redo.last() }).cloned() else {
                    return source_edit("no source history available");
                };
                let scene = self.validate(&raw)?;
                let after = SourceSnapshot::new(raw, &paths::display(&self.source))?;
                self.save(&self.snapshot.clone(), &after)?;
                let previous = self.snapshot.raw.clone();
                if undo {
                    self.undo.pop();
                    self.redo.push(previous);
                } else {
                    self.redo.pop();
                    self.undo.push(previous);
                }
                self.snapshot = after;
                self.accepted = scene;
                self.source_pending = None;
            }
            _ => return source_edit("unsupported definition operation"),
        }
        self.revision += 1;
        self.accepted()
    }
}

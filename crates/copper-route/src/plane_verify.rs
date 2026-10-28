//! Native filled-plane verification records (`pcbir.plane_verify`).
//!
//! The data types of a `kicad-cli` verification, as the routing report and the
//! filled-copper feedback read them. Running `kicad-cli` itself
//! (`verify_filled_planes`, `remove_native_dangling_tracks`) is ported with the
//! filled-copper feedback.

use copper_json::{Dumps, Object, Value};
use copper_pcb::board::PhysicalBoard;
use copper_pcb::board::{DecouplingLink, PadReference};

/// `NativeCopperItem`: exact export UUID binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCopperItem {
    pub uuid: String,
    pub kind: String,
    pub net: Option<String>,
    pub component: Option<String>,
    pub pad: Option<String>,
    pub layers: Vec<String>,
    pub positions_nm: Vec<(i64, i64)>,
    pub region: Option<String>,
    pub index: Option<i64>,
}

/// `NativeFinding`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFinding {
    pub category: String,
    pub kind: String,
    pub description: String,
    pub items: Vec<NativeCopperItem>,
}

/// `NativeFilledRegion`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeFilledRegion {
    pub zone_uuid: String,
    pub fragment_digest: String,
    pub net: String,
    pub layer: String,
    pub vertices_nm: Vec<(i64, i64)>,
}

/// `decoupling_fill.DecouplingFillAssessment`.
#[derive(Clone, Debug, PartialEq)]
pub struct DecouplingFillAssessment {
    pub link: DecouplingLink,
    pub status: String,
    pub supply_connected: Option<bool>,
    pub return_connected: Option<bool>,
    pub return_targets: Vec<PadReference>,
    pub reason: String,
    pub qualification: String,
}

pub const DECOUPLING_FILL_QUALIFICATION: &str = "Physical copper continuity observation only. Local supply/return \
path, loop impedance, component internals and high-frequency performance unqualified.";

/// `PlaneVerification`: one native verification of an exact export.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneVerification {
    pub passed: bool,
    pub board_digest: String,
    pub export_digest: String,
    pub filled_board_digest: String,
    pub report_digest: String,
    pub kicad_version: String,
    pub unconnected_count: i64,
    pub island_count: i64,
    pub other_violation_count: i64,
    pub findings: Vec<String>,
    pub dangling_track_uuids: Vec<String>,
    pub native_findings: Vec<NativeFinding>,
    pub artifact_directory: Option<String>,
    pub filled_regions: Vec<NativeFilledRegion>,
    pub decoupling_assessments: Vec<DecouplingFillAssessment>,
}

fn pairs(items: &[(i64, i64)]) -> Value {
    Value::Array(items.iter().map(|(x, y)| Value::Array(vec![Value::from(*x), Value::from(*y)])).collect())
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn optional_text(value: Option<&String>) -> Value {
    value.map_or(Value::Null, |text| Value::from(text.as_str()))
}

fn pad_reference_document(pad: &PadReference) -> Value {
    let mut document = Object::new();
    document.insert("component".to_owned(), Value::from(pad.component.as_str()));
    document.insert("pad".to_owned(), Value::from(pad.pad.as_str()));
    Value::Object(document)
}

impl NativeCopperItem {
    /// `dataclasses.asdict`.
    pub fn document(&self) -> Value {
        let mut document = Object::new();
        document.insert("uuid".to_owned(), Value::from(self.uuid.as_str()));
        document.insert("kind".to_owned(), Value::from(self.kind.as_str()));
        document.insert("net".to_owned(), optional_text(self.net.as_ref()));
        document.insert("component".to_owned(), optional_text(self.component.as_ref()));
        document.insert("pad".to_owned(), optional_text(self.pad.as_ref()));
        document.insert("layers".to_owned(), strings(&self.layers));
        document.insert("positions_nm".to_owned(), pairs(&self.positions_nm));
        document.insert("region".to_owned(), optional_text(self.region.as_ref()));
        document.insert("index".to_owned(), self.index.map_or(Value::Null, Value::from));
        Value::Object(document)
    }
}

impl PlaneVerification {
    /// `matches(board)`: the evidence is about this exact board and export.
    pub fn matches(&self, board: &PhysicalBoard) -> copper_pcb::Result<bool> {
        if self.board_digest != copper_drc::digest::physical_board_digest(board) {
            return Ok(false);
        }
        let manifest = copper_pcb::kicad_pcb::generate(board)?;
        Ok(self.export_digest == kicad_export_digest(&manifest))
    }

    /// `zone_connectivity_verified(board)`.
    pub fn zone_connectivity_verified(&self, board: &PhysicalBoard) -> copper_pcb::Result<bool> {
        let library = |item: &String| {
            let kind = item.split_once(':').map_or(item.as_str(), |(kind, _)| kind);
            kind == "lib_footprint_issues" || kind == "lib_footprint_mismatch"
        };
        Ok(self.unconnected_count == 0
            && self.island_count == 0
            && self.other_violation_count == self.findings.len() as i64
            && self.findings.iter().all(library)
            && self.matches(board)?)
    }

    /// The `to_json` document.
    pub fn document(&self) -> Value {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from("copperscript-plane-verification/v0.1"));
        document.insert("passed".to_owned(), Value::from(self.passed));
        document.insert("board_digest".to_owned(), Value::from(self.board_digest.as_str()));
        document.insert("export_digest".to_owned(), Value::from(self.export_digest.as_str()));
        document.insert("filled_board_digest".to_owned(), Value::from(self.filled_board_digest.as_str()));
        document.insert("report_digest".to_owned(), Value::from(self.report_digest.as_str()));
        document.insert("kicad_version".to_owned(), Value::from(self.kicad_version.as_str()));
        document.insert("unconnected_count".to_owned(), Value::from(self.unconnected_count));
        document.insert("island_count".to_owned(), Value::from(self.island_count));
        document.insert("other_violation_count".to_owned(), Value::from(self.other_violation_count));
        document.insert("findings".to_owned(), strings(&self.findings));
        document.insert("dangling_track_uuids".to_owned(), strings(&self.dangling_track_uuids));
        let native_findings = self
            .native_findings
            .iter()
            .map(|finding| {
                let mut item = Object::new();
                item.insert("category".to_owned(), Value::from(finding.category.as_str()));
                item.insert("type".to_owned(), Value::from(finding.kind.as_str()));
                item.insert("description".to_owned(), Value::from(finding.description.as_str()));
                item.insert(
                    "items".to_owned(),
                    Value::Array(finding.items.iter().map(NativeCopperItem::document).collect()),
                );
                Value::Object(item)
            })
            .collect();
        document.insert("native_findings".to_owned(), Value::Array(native_findings));
        document.insert("artifact_directory".to_owned(), optional_text(self.artifact_directory.as_ref()));
        let regions = self
            .filled_regions
            .iter()
            .map(|region| {
                let mut item = Object::new();
                item.insert("zone_uuid".to_owned(), Value::from(region.zone_uuid.as_str()));
                item.insert("fragment_digest".to_owned(), Value::from(region.fragment_digest.as_str()));
                item.insert("net".to_owned(), Value::from(region.net.as_str()));
                item.insert("layer".to_owned(), Value::from(region.layer.as_str()));
                item.insert("vertices_nm".to_owned(), pairs(&region.vertices_nm));
                Value::Object(item)
            })
            .collect();
        document.insert("filled_regions".to_owned(), Value::Array(regions));
        let assessments = self
            .decoupling_assessments
            .iter()
            .map(|assessment| {
                let mut link = Object::new();
                link.insert("capacitor".to_owned(), pad_reference_document(&assessment.link.capacitor));
                link.insert("target".to_owned(), pad_reference_document(&assessment.link.target));
                link.insert("return_pad".to_owned(), pad_reference_document(&assessment.link.return_pad));
                link.insert("net".to_owned(), Value::from(assessment.link.net.as_str()));
                link.insert("return_net".to_owned(), Value::from(assessment.link.return_net.as_str()));
                let mut item = Object::new();
                item.insert("link".to_owned(), Value::Object(link));
                item.insert("status".to_owned(), Value::from(assessment.status.as_str()));
                item.insert(
                    "supply_connected".to_owned(),
                    assessment.supply_connected.map_or(Value::Null, Value::from),
                );
                item.insert(
                    "return_connected".to_owned(),
                    assessment.return_connected.map_or(Value::Null, Value::from),
                );
                item.insert(
                    "return_targets".to_owned(),
                    Value::Array(assessment.return_targets.iter().map(pad_reference_document).collect()),
                );
                item.insert("reason".to_owned(), Value::from(assessment.reason.as_str()));
                item.insert("qualification".to_owned(), Value::from(assessment.qualification.as_str()));
                Value::Object(item)
            })
            .collect();
        document.insert("decoupling_assessments".to_owned(), Value::Array(assessments));
        Value::Object(document)
    }

    /// `to_json()`.
    pub fn to_json(&self) -> String {
        Dumps::new().indent(2).sort_keys(true).dumps(&self.document()) + "\n"
    }
}

/// `kicad_project.kicad_export_digest(manifest)`: binds verification to every exported asset.
pub fn kicad_export_digest(manifest: &copper_pcb::kicad_pcb::Manifest) -> String {
    use sha2::{Digest, Sha256};
    let hex = |bytes: &[u8]| -> String { Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect() };
    let document = Value::Array(
        manifest
            .artifacts
            .iter()
            .map(|item| Value::Array(vec![Value::from(item.name.as_str()), Value::from(hex(item.content.as_bytes()))]))
            .collect(),
    );
    hex(Dumps::new().separators(",", ":").dumps(&document).as_bytes())
}

// ---------------------------------------------------------------------------
// Running kicad-cli (`native_object_map`, `verify_filled_planes`)

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

/// An exception `verify_filled_planes` raises, as Python's `str(exc)`:
/// `RuntimeError`, `OSError` (a missing tool is `FileNotFoundError`), or a
/// pipeline error.
#[derive(Debug)]
pub enum NativeError {
    Runtime(String),
    Os { missing: bool, message: String },
    Pcb(copper_pcb::Error),
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeError::Runtime(message) | NativeError::Os { message, .. } => formatter.write_str(message),
            NativeError::Pcb(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<copper_pcb::Error> for NativeError {
    fn from(error: copper_pcb::Error) -> Self {
        NativeError::Pcb(error)
    }
}

fn runtime<T>(message: impl Into<String>) -> Result<T, NativeError> {
    Err(NativeError::Runtime(message.into()))
}

fn os_error(error: &std::io::Error, filename: Option<&str>) -> NativeError {
    NativeError::Os {
        missing: error.kind() == std::io::ErrorKind::NotFound,
        message: copper_json::oserror::python_os_error(error, filename, copper_json::oserror::Call::Os),
    }
}

/// `_stable_uuid`-bound native objects of the export of `board`.
pub fn native_object_map(board: &PhysicalBoard) -> copper_pcb::Result<IndexMap<String, NativeCopperItem>> {
    use copper_physical::footprint::PadKind;
    use copper_physical::{BoardSide, CopperLayer};
    let mut assigned: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            assigned.insert((pad.component.as_str(), pad.pad.as_str()), net.name.as_str());
        }
    }
    let mut objects = IndexMap::new();
    let mut add = |kind: &str, parts: &[&str], item: NativeCopperItem| {
        let mut all = vec![board.name.as_str(), kind];
        all.extend_from_slice(parts);
        let uuid = copper_pcb::kicad_pcb::stable_uuid(&all);
        objects.insert(uuid.clone(), NativeCopperItem { uuid, kind: kind.to_owned(), ..item });
    };
    let blank = || NativeCopperItem {
        uuid: String::new(),
        kind: String::new(),
        net: None,
        component: None,
        pad: None,
        layers: Vec::new(),
        positions_nm: Vec::new(),
        region: None,
        index: None,
    };
    let values = |layers: &[CopperLayer]| layers.iter().map(|l| l.value().to_owned()).collect::<Vec<_>>();
    for pose in &board.placements {
        for (index, pad) in board.footprints[&pose.footprint].pads.iter().enumerate() {
            let position = copper_pcb::transform::transformed_local_point(pose, pad.position);
            let layers = if pad.kind == PadKind::Smd {
                vec![if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                board.stackup.copper_layers.clone()
            };
            let number = index.to_string();
            add(
                "pad",
                &[&pose.reference, &number, &pad.number],
                NativeCopperItem {
                    net: assigned.get(&(pose.reference.as_str(), pad.number.as_str())).map(|n| (*n).to_owned()),
                    component: Some(pose.reference.clone()),
                    pad: Some(pad.number.clone()),
                    layers: values(&layers),
                    positions_nm: vec![(position.x_nm, position.y_nm)],
                    index: Some(index as i64),
                    ..blank()
                },
            );
        }
    }
    for (index, track) in board.tracks.iter().enumerate() {
        add(
            "segment",
            &[&index.to_string()],
            NativeCopperItem {
                net: Some(track.net.clone()),
                layers: vec![track.layer.value().to_owned()],
                index: Some(index as i64),
                positions_nm: vec![(track.start.x_nm, track.start.y_nm), (track.end.x_nm, track.end.y_nm)],
                ..blank()
            },
        );
    }
    let stack = &board.stackup.copper_layers;
    for (index, via) in board.vias.iter().enumerate() {
        let position = |layer| stack.iter().position(|l| *l == layer).expect("via layer");
        let (a, b) = (position(via.from_layer), position(via.to_layer));
        let (first, last) = (a.min(b), a.max(b));
        add(
            "via",
            &[&index.to_string()],
            NativeCopperItem {
                net: Some(via.net.clone()),
                index: Some(index as i64),
                layers: values(&stack[first..=last]),
                positions_nm: vec![(via.position.x_nm, via.position.y_nm)],
                ..blank()
            },
        );
    }
    for zone in &board.zones {
        for layer in &zone.layers {
            add(
                "zone",
                &[&zone.id, layer.value()],
                NativeCopperItem {
                    net: Some(zone.net.clone()),
                    layers: vec![layer.value().to_owned()],
                    region: Some(zone.id.clone()),
                    positions_nm: zone.outer.iter().map(|p| (p.x_nm, p.y_nm)).collect(),
                    ..blank()
                },
            );
        }
    }
    Ok(objects)
}

/// `_filled_regions`: the filled polygons of a native board, bound to source zones.
fn filled_regions(
    pcb_text: &str,
    objects: &IndexMap<String, NativeCopperItem>,
) -> Result<Vec<NativeFilledRegion>, NativeError> {
    use copper_physical::kicad_mod::{atom, children, first, parse_sexpr};
    use sha2::{Digest, Sha256};
    let malformed = || NativeError::Runtime("unsupported or malformed native filled polygons".to_owned());
    let root = parse_sexpr(pcb_text, "native filled board").map_err(|_| malformed())?;
    let mut regions = Vec::new();
    for zone in children(&root, "zone") {
        let uuid = first(zone, "uuid").and_then(|node| atom(node, 1)).map(str::to_owned);
        let binding = uuid.as_ref().and_then(|uuid| objects.get(uuid));
        for polygon in children(zone, "filled_polygon") {
            let Some(binding) = binding.filter(|b| b.kind == "zone" && b.net.is_some()) else {
                return runtime("native filled polygon has no exact source-zone UUID binding");
            };
            let layer = first(polygon, "layer").and_then(|node| atom(node, 1));
            if !layer.is_some_and(|layer| binding.layers.iter().any(|l| l == layer)) {
                return runtime("native filled polygon has an unexpected layer");
            }
            let layer = layer.expect("layer").to_owned();
            let Some(points) = first(polygon, "pts") else {
                return runtime("native filled polygon lacks vertices");
            };
            let mut vertices = Vec::new();
            for point in children(points, "xy") {
                let coordinate = |index| {
                    atom(point, index)
                        .ok_or_else(malformed)
                        .and_then(|text| copper_pcb::nm_from_mm_text(text).map_err(|_| malformed()))
                };
                vertices.push((coordinate(1)?, coordinate(2)?));
            }
            if vertices.len() < 3 {
                return runtime("native filled polygon has insufficient vertices");
            }
            let uuid = uuid.clone().expect("bound uuid");
            let listed: Vec<String> = vertices.iter().map(|(x, y)| format!("({x}, {y})")).collect();
            let text = format!(
                "({}, {}, ({}))",
                copper_json::python_str_repr(&uuid),
                copper_json::python_str_repr(&layer),
                listed.join(", ")
            );
            let digest: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            regions.push(NativeFilledRegion {
                zone_uuid: uuid,
                fragment_digest: digest,
                net: binding.net.clone().expect("zone net"),
                layer,
                vertices_nm: vertices,
            });
        }
    }
    Ok(regions)
}

struct Completed {
    code: i32,
    stdout: String,
    stderr: String,
}

/// `manufacturing._subprocess_runner`: `subprocess.run(text=True, capture_output=True)`.
fn run(command: &[String], cwd: &Path) -> Result<Completed, NativeError> {
    let output = std::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(cwd)
        .output()
        .map_err(|failure| os_error(&failure, (!cfg!(windows)).then_some(command[0].as_str())))?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n").replace('\r', "\n");
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        output.status.code().unwrap_or_else(|| -output.status.signal().unwrap_or(1))
    };
    #[cfg(not(unix))]
    let code = output.status.code().unwrap_or(1);
    Ok(Completed { code, stdout: text(&output.stdout), stderr: text(&output.stderr) })
}

fn strip(text: &str) -> &str {
    text.trim_matches(copper_json::python_isspace)
}

fn violation_type(item: &Value) -> Result<String, NativeError> {
    let Some(object) = item.as_object() else { return runtime("malformed KiCad DRC finding") };
    match object.get("type") {
        None => Ok("unknown".to_owned()),
        Some(Value::Str(text)) => Ok(text.clone()),
        Some(_) => {
            Err(NativeError::Pcb(copper_pcb::Error::NotImplemented("a non-string KiCad DRC finding type".to_owned())))
        }
    }
}

fn description(item: &Value) -> Result<String, NativeError> {
    match item.get("description") {
        None => Ok(String::new()),
        Some(Value::Str(text)) => Ok(text.clone()),
        Some(_) => Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(
            "a non-string KiCad DRC finding description".to_owned(),
        ))),
    }
}

/// `verify_filled_planes`: refill a private KiCad board and require zero
/// DRC/connectivity findings. `artifact_directory` must not exist yet.
pub fn verify_filled_planes(
    board: &PhysicalBoard,
    kicad_cli: &Path,
    qualified_major: i64,
    artifact_directory: Option<&Path>,
) -> Result<PlaneVerification, NativeError> {
    use sha2::{Digest, Sha256};
    let hex = |bytes: &[u8]| -> String { Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() };
    let (stage, _temporary): (PathBuf, Option<TemporaryDirectory>) = match artifact_directory {
        Some(directory) => {
            let resolved = copper_lang::paths::resolve(directory);
            if resolved.exists() {
                let failure = std::io::Error::from_raw_os_error(17);
                return Err(os_error(&failure, Some(&copper_lang::paths::display(&resolved))));
            }
            std::fs::create_dir_all(&resolved)
                .map_err(|failure| os_error(&failure, Some(&copper_lang::paths::display(&resolved))))?;
            (resolved, None)
        }
        None => {
            let temporary = TemporaryDirectory::new("copperscript-plane-")?;
            (temporary.path.clone(), Some(temporary))
        }
    };
    let cli = copper_lang::paths::display(kicad_cli);
    let version_result = run(&[cli.clone(), "--version".to_owned()], &stage)?;
    if version_result.code != 0 {
        return runtime(format!("KiCad version query failed: {}", version_result.stderr));
    }
    let version = strip(&version_result.stdout).lines().next().unwrap_or("").to_owned();
    let major: Option<i64> = {
        let digits: String = version.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() { None } else { digits.parse().ok() }
    };
    if major != Some(qualified_major) {
        return runtime(format!(
            "KiCad {} is not the qualified major {qualified_major}",
            copper_json::python_str_repr(&version)
        ));
    }
    let manifest = copper_pcb::kicad_pcb::generate(board)?;
    if manifest.artifacts.len() < 2 {
        return runtime("KiCad backend omitted the same-stem project");
    }
    let pcb_path = stage.join(&manifest.artifacts[0].name);
    copper_pcb::project::write_kicad_project(&manifest, &pcb_path)
        .map_err(|message| NativeError::Pcb(copper_pcb::Error::Value(message)))?;
    if artifact_directory.is_some() {
        let original = stage.join("input");
        std::fs::create_dir(&original)
            .map_err(|failure| os_error(&failure, Some(&copper_lang::paths::display(&original))))?;
        copper_pcb::project::write_kicad_project(&manifest, &original.join(&manifest.artifacts[0].name))
            .map_err(|message| NativeError::Pcb(copper_pcb::Error::Value(message)))?;
    }
    let export_digest = kicad_export_digest(&manifest);
    let report_path = stage.join("drc.json");
    let mut command: Vec<String> = [
        cli.as_str(),
        "pcb",
        "drc",
        "--format",
        "json",
        "--severity-all",
        "--output",
        &copper_lang::paths::display(&report_path),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if !board.zones.is_empty() {
        command.extend(["--refill-zones".to_owned(), "--save-board".to_owned()]);
    }
    command.push(copper_lang::paths::display(&pcb_path));
    let result = run(&command, &stage)?;
    if result.code != 0 || !report_path.is_file() {
        let detail = [strip(&result.stderr), strip(&result.stdout)].into_iter().find(|t| !t.is_empty());
        return runtime(format!("KiCad refill/DRC failed: {}", detail.unwrap_or("no report")));
    }
    let report_bytes = std::fs::read(&report_path)
        .map_err(|failure| os_error(&failure, Some(&copper_lang::paths::display(&report_path))))?;
    let Ok(text) = std::str::from_utf8(report_bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&report_bytes)) else {
        return runtime("KiCad produced an invalid JSON DRC report");
    };
    let Ok(report) = copper_json::parse(text) else { return runtime("KiCad produced an invalid JSON DRC report") };
    let Some(report) = report.as_object() else { return runtime("KiCad produced a non-object DRC report") };
    let (Some(Value::Array(violations)), Some(Value::Array(unconnected))) =
        (report.get("violations"), report.get("unconnected_items"))
    else {
        return runtime("KiCad DRC report lacks violations/unconnected_items");
    };
    let mut islands = Vec::new();
    let mut others = Vec::new();
    for item in violations {
        if violation_type(item)? == "isolated_copper" { islands.push(item) } else { others.push(item) }
    }
    let mut findings = Vec::new();
    for item in unconnected.iter().chain(islands.iter().copied()).chain(others.iter().copied()) {
        findings.push(format!("{}: {}", violation_type(item)?, description(item)?));
    }
    let mut dangling: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for item in &others {
        if violation_type(item)? != "track_dangling" {
            continue;
        }
        if let Some(Value::Array(entries)) = item.get("items") {
            for entry in entries {
                if let Some(Value::Str(uuid)) = entry.as_object().and_then(|o| o.get("uuid")) {
                    dangling.insert(uuid.clone());
                }
            }
        } else if item.get("items").is_some() {
            return Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(
                "non-list KiCad DRC finding items".to_owned(),
            )));
        }
    }
    let objects = native_object_map(board)?;
    let mut mapped = Vec::new();
    for (category, entries) in [("unconnected", unconnected), ("violation", violations)] {
        for item in entries {
            let raw = match item.get("items") {
                None => &[][..],
                Some(Value::Array(raw)) => &raw[..],
                Some(_) => return runtime("malformed KiCad DRC objects"),
            };
            let mut bindings = Vec::new();
            for entry in raw {
                let Some(Value::Str(uuid)) = entry.as_object().and_then(|o| o.get("uuid")) else {
                    return runtime("malformed KiCad DRC object UUID");
                };
                bindings.push(objects.get(uuid).cloned().unwrap_or_else(|| NativeCopperItem {
                    uuid: uuid.clone(),
                    kind: "unknown".to_owned(),
                    net: None,
                    component: None,
                    pad: None,
                    layers: Vec::new(),
                    positions_nm: Vec::new(),
                    region: None,
                    index: None,
                }));
            }
            mapped.push(NativeFinding {
                category: category.to_owned(),
                kind: violation_type(item)?,
                description: description(item)?,
                items: bindings,
            });
        }
    }
    let pcb_bytes = std::fs::read(&pcb_path)
        .map_err(|failure| os_error(&failure, Some(&copper_lang::paths::display(&pcb_path))))?;
    let filled = if board.zones.is_empty() {
        Vec::new()
    } else {
        let Ok(text) = std::str::from_utf8(&pcb_bytes) else {
            return runtime("unsupported or malformed native filled polygons");
        };
        filled_regions(text, &objects)?
    };
    if !board.decoupling_links.is_empty() {
        return Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(
            "native decoupling fill assessment (decoupling_fill.assess_decoupling_fill)".to_owned(),
        )));
    }
    Ok(PlaneVerification {
        passed: findings.is_empty(),
        board_digest: copper_drc::digest::physical_board_digest(board),
        export_digest,
        filled_board_digest: hex(&pcb_bytes),
        report_digest: hex(&report_bytes),
        kicad_version: version,
        unconnected_count: unconnected.len() as i64,
        island_count: islands.len() as i64,
        other_violation_count: others.len() as i64,
        findings,
        dangling_track_uuids: dangling.into_iter().collect(),
        native_findings: mapped,
        artifact_directory: artifact_directory.map(|_| copper_lang::paths::display(&stage)),
        filled_regions: filled,
        decoupling_assessments: Vec::new(),
    })
}

/// `tempfile.TemporaryDirectory(prefix=...)`, removed on drop.
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(prefix: &str) -> Result<Self, NativeError> {
        let base = std::env::temp_dir();
        for attempt in 0..100u32 {
            let nanos =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let path = base.join(format!("{prefix}{:x}{attempt}", nanos ^ std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TemporaryDirectory { path }),
                Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(failure) => return Err(os_error(&failure, Some(&copper_lang::paths::display(&path)))),
            }
        }
        runtime("cannot create a temporary directory")
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

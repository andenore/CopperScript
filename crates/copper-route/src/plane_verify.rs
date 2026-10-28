//! Native filled-plane verification records (`pcbir.plane_verify`).
//!
//! The data types of a `kicad-cli` verification, as the routing report and the
//! filled-copper feedback read them. Running `kicad-cli` itself
//! (`verify_filled_planes`, `remove_native_dangling_tracks`) is ported with the
//! filled-copper feedback.

use copper_json::{Dumps, Object, Value};
use copper_pcb::board::{DecouplingLink, PadReference};
use copper_pcb::board::PhysicalBoard;

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
                item.insert("items".to_owned(), Value::Array(finding.items.iter().map(NativeCopperItem::document).collect()));
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
                item.insert("supply_connected".to_owned(), assessment.supply_connected.map_or(Value::Null, Value::from));
                item.insert("return_connected".to_owned(), assessment.return_connected.map_or(Value::Null, Value::from));
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

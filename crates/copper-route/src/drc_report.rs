//! The physical DRC report document and signoff token (`pcbir.drc`:
//! `PhysicalDrcReport.to_json`, `SignoffToken`, `DrcFinding.fingerprint`).
//!
//! `copper-drc` checks the board and returns findings and coverage; the
//! routing pipeline and its report need the complete document the Python
//! report writes, with each finding's fingerprint, the completeness verdict
//! and the token binding the report to the board digest. Waivers are never
//! given by the routing commands, so every finding is `active`. Component
//! hole-clearance relaxation records are not modelled by the Rust DRC; a
//! board that could produce them reports not implemented rather than an
//! incomplete document.

use copper_drc::digest::{json_digest, physical_board_digest};
use copper_drc::{BreakoutRelaxation, Coverage, CoverageStatus, Decision, Finding, Policy, Report};
use copper_json::{Dumps, Object, Value};
use copper_pcb::Result;
use copper_pcb::board::PhysicalBoard;

pub const SCHEMA: &str = "copperscript-physical-drc/v0.1";
pub const TOKEN_SCHEMA: &str = "copperscript-signoff/v0.1";

/// `DrcCoverageStatus.value`.
pub fn coverage_status_value(status: CoverageStatus) -> &'static str {
    match status {
        CoverageStatus::Executed => "executed",
        CoverageStatus::NotApplicable => "not_applicable",
        CoverageStatus::Unsupported => "unsupported",
        CoverageStatus::Failed => "failed",
    }
}

/// `DrcDecision.value` (no waivers are ever given here).
pub fn decision_value(decision: Decision) -> &'static str {
    match decision {
        Decision::Pass => "pass",
        Decision::Fail => "fail",
    }
}

fn completeness_value(complete: bool) -> &'static str {
    if complete { "complete" } else { "incomplete" }
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn optional(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// `DrcFinding.fingerprint`.
pub fn fingerprint(finding: &Finding) -> String {
    let mut document = Object::new();
    document.insert("code".to_owned(), Value::from(finding.code.as_str()));
    document.insert("message".to_owned(), Value::from(finding.message.as_str()));
    document.insert("objects".to_owned(), strings(&finding.objects));
    document.insert("nets".to_owned(), strings(&finding.nets));
    document.insert("layers".to_owned(), strings(&finding.layers));
    document.insert("required_nm".to_owned(), optional(finding.required_nm));
    document.insert("measured_nm".to_owned(), optional(finding.measured_nm));
    json_digest(&Value::Object(document))
}

fn finding_document(finding: &Finding) -> Value {
    let mut document = Object::new();
    document.insert("fingerprint".to_owned(), Value::from(fingerprint(finding)));
    document.insert("code".to_owned(), Value::from(finding.code.as_str()));
    document.insert("severity".to_owned(), Value::from(finding.severity.value()));
    document.insert("message".to_owned(), Value::from(finding.message.as_str()));
    document.insert("objects".to_owned(), strings(&finding.objects));
    document.insert("nets".to_owned(), strings(&finding.nets));
    document.insert("layers".to_owned(), strings(&finding.layers));
    document.insert("required_nm".to_owned(), optional(finding.required_nm));
    document.insert("measured_nm".to_owned(), optional(finding.measured_nm));
    document.insert("disposition".to_owned(), Value::from("active"));
    document.insert("waiver_reason".to_owned(), Value::Null);
    Value::Object(document)
}

fn relaxation_document(item: &BreakoutRelaxation) -> Value {
    let mut document = Object::new();
    document.insert("check".to_owned(), Value::from(item.check.as_str()));
    document.insert("objects".to_owned(), strings(&item.objects));
    document.insert("nets".to_owned(), strings(&item.nets));
    document.insert("layers".to_owned(), strings(&item.layers));
    document.insert("required_nm".to_owned(), Value::from(item.required_nm));
    document.insert("relaxed_nm".to_owned(), Value::from(item.relaxed_nm));
    document.insert("measured_nm".to_owned(), optional(item.measured_nm));
    let regions = item
        .regions
        .iter()
        .map(|(net, land, length)| {
            let mut region = Object::new();
            region.insert("net".to_owned(), Value::from(net.as_str()));
            region.insert("land".to_owned(), Value::from(land.as_str()));
            region.insert("breakout_length_nm".to_owned(), Value::from(*length));
            Value::Object(region)
        })
        .collect();
    document.insert("regions".to_owned(), Value::Array(regions));
    Value::Object(document)
}

fn coverage_document(item: &Coverage) -> Value {
    let mut document = Object::new();
    document.insert("check".to_owned(), Value::from(item.check.as_str()));
    document.insert("status".to_owned(), Value::from(coverage_status_value(item.status)));
    document.insert("required".to_owned(), Value::from(item.required));
    document.insert("detail".to_owned(), Value::from(item.detail.as_str()));
    Value::Object(document)
}

/// `SignoffToken`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignoffToken {
    pub schema: String,
    pub board_digest: String,
    pub rules_digest: String,
    pub report_digest: String,
    pub decision: Decision,
    pub complete: bool,
}

impl SignoffToken {
    /// `SignoffToken.token_digest`.
    pub fn token_digest(&self) -> String {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from(self.schema.as_str()));
        document.insert("board_digest".to_owned(), Value::from(self.board_digest.as_str()));
        document.insert("rules_digest".to_owned(), Value::from(self.rules_digest.as_str()));
        document.insert("report_digest".to_owned(), Value::from(self.report_digest.as_str()));
        document.insert("decision".to_owned(), Value::from(decision_value(self.decision)));
        document.insert("completeness".to_owned(), Value::from(completeness_value(self.complete)));
        json_digest(&Value::Object(document))
    }

    fn document(&self) -> Value {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from(self.schema.as_str()));
        document.insert("board_digest".to_owned(), Value::from(self.board_digest.as_str()));
        document.insert("rules_digest".to_owned(), Value::from(self.rules_digest.as_str()));
        document.insert("report_digest".to_owned(), Value::from(self.report_digest.as_str()));
        document.insert("decision".to_owned(), Value::from(decision_value(self.decision)));
        document.insert("completeness".to_owned(), Value::from(completeness_value(self.complete)));
        document.insert("token_digest".to_owned(), Value::from(self.token_digest()));
        Value::Object(document)
    }
}

/// `PhysicalDrcReport` with its token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrcReport {
    pub report: Report,
    pub token: SignoffToken,
}

impl DrcReport {
    pub fn decision(&self) -> Decision {
        self.report.decision
    }

    pub fn findings(&self) -> &[Finding] {
        &self.report.findings
    }

    /// `_report_document(report, include_token=...)`.
    pub fn document(&self, include_token: bool) -> Value {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from(SCHEMA));
        document.insert("decision".to_owned(), Value::from(decision_value(self.report.decision)));
        document.insert("completeness".to_owned(), Value::from(completeness_value(self.report.complete)));
        document
            .insert("coverage".to_owned(), Value::Array(self.report.coverage.iter().map(coverage_document).collect()));
        document
            .insert("findings".to_owned(), Value::Array(self.report.findings.iter().map(finding_document).collect()));
        if !self.report.breakout_relaxations.is_empty() {
            // Present only when breakout values relaxed a check.
            let items = self.report.breakout_relaxations.iter().map(relaxation_document).collect();
            document.insert("breakout_relaxations".to_owned(), Value::Array(items));
        }
        if include_token {
            document.insert("token".to_owned(), self.token.document());
        }
        Value::Object(document)
    }

    /// `PhysicalDrcReport.to_json()`.
    pub fn to_json(&self) -> String {
        Dumps::new().indent(2).sort_keys(true).dumps(&self.document(true)) + "\n"
    }
}

/// `_rules_digest`.
fn rules_digest(board_digest: &str, policy: Policy) -> String {
    let mut inner = Object::new();
    inner.insert("require_completed_detailed_route".to_owned(), Value::from(policy.require_completed_detailed_route));
    inner.insert("fail_on_warnings".to_owned(), Value::from(policy.fail_on_warnings));
    let mut document = Object::new();
    document.insert("physical".to_owned(), Value::from(board_digest));
    document.insert("policy".to_owned(), Value::Object(inner));
    json_digest(&Value::Object(document))
}

/// `run_physical_drc(board, policy=...)` with the report bound to its token.
pub fn run_physical_drc(board: &PhysicalBoard, policy: Policy) -> Result<DrcReport> {
    if !board.component_hole_clearances.is_empty() {
        return copper_pcb::not_implemented("DRC component hole-clearance relaxation records");
    }
    let report = copper_drc::run_physical_drc(board, policy)?;
    let board_digest = physical_board_digest(board);
    let token = SignoffToken {
        schema: TOKEN_SCHEMA.to_owned(),
        board_digest: board_digest.clone(),
        rules_digest: rules_digest(&board_digest, policy),
        report_digest: String::new(),
        decision: report.decision,
        complete: report.complete,
    };
    let mut bound = DrcReport { report, token };
    bound.token.report_digest = json_digest(&bound.document(false));
    Ok(bound)
}

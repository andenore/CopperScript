//! Native filled-copper feedback (`pcbir.filled_feedback`): refill first, and
//! repair only verified opens, each trial refilled independently.
//!
//! The verification outcomes are rewritten; the bounded bridge/dependency
//! repair trials (`reroute_local_dependencies`) exit 3.

use std::path::Path;

use copper_json::{Dumps, Object, Value};
use copper_pcb::board::{PhysicalBoard, RouteKind};

use crate::flow::RoutingPipelineResult;
use crate::plane_verify::{NativeError, PlaneVerification, verify_filled_planes};

/// `FilledCopperFeedbackOptions` (the bounds the command line sets).
#[derive(Clone, Debug)]
pub struct FilledCopperFeedbackOptions {
    pub maximum_trials: i64,
    pub maximum_search_states: i64,
    pub maximum_blockers: i64,
    pub maximum_dependency_expansions: i64,
}

impl FilledCopperFeedbackOptions {
    /// `__post_init__` for the fields set here (the others keep Python's defaults).
    pub fn validate(&self) -> copper_pcb::Result<()> {
        if self.maximum_trials < 0
            || self.maximum_search_states <= 0
            || !(1..=8).contains(&self.maximum_blockers)
            || !(0..=8).contains(&self.maximum_dependency_expansions)
        {
            return Err(copper_pcb::Error::Value("filled-copper feedback bounds are invalid".to_owned()));
        }
        Ok(())
    }
}

/// `FilledCopperFeedbackResult` of a run without trials.
#[derive(Clone, Debug)]
pub struct FilledCopperFeedbackResult {
    pub board: PhysicalBoard,
    pub verification: Option<PlaneVerification>,
    pub stop_reason: String,
    pub diagnostic: Option<String>,
}

impl FilledCopperFeedbackResult {
    pub fn to_json(&self) -> String {
        let mut document = Object::new();
        document.insert("schema".to_owned(), Value::from("copperscript-filled-feedback/v0.1"));
        document.insert("stop_reason".to_owned(), Value::from(self.stop_reason.as_str()));
        document.insert("diagnostic".to_owned(), self.diagnostic.as_deref().map_or(Value::Null, Value::from));
        document.insert("board_digest".to_owned(), Value::from(copper_drc::digest::physical_board_digest(&self.board)));
        document.insert(
            "verification".to_owned(),
            self.verification.as_ref().map_or(Value::Null, PlaneVerification::document),
        );
        document.insert("trials".to_owned(), Value::Array(Vec::new()));
        Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n"
    }
}

/// `_hard_findings`: physical DRC errors other than opens.
fn hard_findings(board: &PhysicalBoard) -> copper_pcb::Result<bool> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default())?;
    Ok(report.report.findings.iter().any(|item| {
        item.severity == copper_drc::Severity::Error
            && item.code != "DRC-OPEN-NET"
            && item.code != "DRC-ROUTE-INCOMPLETE"
    }))
}

/// `_open_nets`: the nets of mapped native opens, or None.
fn open_nets(evidence: &PlaneVerification) -> Option<indexmap::IndexSet<String>> {
    let entries: Vec<_> = evidence.native_findings.iter().filter(|f| f.category == "unconnected").collect();
    if entries.len() as i64 != evidence.unconnected_count
        || entries.iter().any(|finding| {
            finding.kind != "unconnected_items"
                || finding.items.is_empty()
                || finding.items.iter().any(|item| item.net.is_none() || item.kind == "unknown")
        })
    {
        return None;
    }
    Some(entries.iter().flat_map(|f| f.items.iter().filter_map(|item| item.net.clone())).collect())
}

/// `improve_filled_copper`: verify the routed board natively; repair trials
/// are not rewritten.
pub fn improve_filled_copper(
    initial: &RoutingPipelineResult,
    board: &PhysicalBoard,
    kicad_cli: &Path,
    options: &FilledCopperFeedbackOptions,
    artifact_directory: Option<&Path>,
) -> Result<FilledCopperFeedbackResult, NativeError> {
    let source = initial.board();
    let mut intent = board.clone();
    intent.tracks = source.tracks.clone();
    intent.vias = source.vias.clone();
    intent.metadata = source.metadata.clone();
    if board.placements != source.placements || intent != *source {
        return Err(NativeError::Pcb(copper_pcb::Error::Value(
            "filled feedback requires matching routed source intent and poses".to_owned(),
        )));
    }
    if let Some(directory) = artifact_directory {
        let display = copper_lang::paths::display(directory);
        if directory.exists() {
            let failure = std::io::Error::from_raw_os_error(17);
            return Err(NativeError::Os {
                missing: false,
                message: copper_json::oserror::python_os_error(
                    &failure,
                    Some(&display),
                    copper_json::oserror::Call::Os,
                ),
            });
        }
        std::fs::create_dir_all(directory).map_err(|failure| NativeError::Os {
            missing: false,
            message: copper_json::oserror::python_os_error(&failure, Some(&display), copper_json::oserror::Call::Os),
        })?;
    }
    let finish = |verification: Option<PlaneVerification>, reason: &str, diagnostic: Option<String>| {
        Ok(FilledCopperFeedbackResult {
            board: board.clone(),
            verification,
            stop_reason: reason.to_owned(),
            diagnostic,
        })
    };
    let directory = artifact_directory.map(|d| d.join("verification-001"));
    let evidence = match verify_filled_planes(board, kicad_cli, 10, directory.as_deref()) {
        Ok(evidence) => evidence,
        Err(NativeError::Pcb(error)) => return Err(NativeError::Pcb(error)),
        Err(error) => {
            let message = error.to_string();
            let reason = match &error {
                NativeError::Os { missing: true, .. } => "missing_tool",
                _ if message.contains("qualified major") => "unsupported_tool",
                _ => "invalid_native_result",
            };
            return finish(None, reason, Some(message));
        }
    };
    if evidence.passed {
        return finish(Some(evidence), "verified", None);
    }
    if hard_findings(board)? {
        return finish(Some(evidence), "physical_violations", None);
    }
    if evidence.island_count != 0 {
        return finish(
            Some(evidence),
            "source_zone_design",
            Some("Native isolated copper requires source-zone review; routing intent is unchanged.".to_owned()),
        );
    }
    if evidence.unconnected_count == 0 {
        return finish(Some(evidence), "native_violations", None);
    }
    let Some(known_opens) = open_nets(&evidence) else {
        return finish(Some(evidence), "unsupported_native_objects", None);
    };
    if options.maximum_trials == 0 {
        return finish(Some(evidence), "disabled", None);
    }
    let zone_nets = copper_pcb::zones::distribution_zone_nets(board);
    let specialized: indexmap::IndexSet<&str> =
        board.net_routing_rules.iter().filter(|r| r.kind != RouteKind::General).map(|r| r.net.as_str()).collect();
    if !known_opens.iter().any(|net| zone_nets.contains(net) && !specialized.contains(net.as_str())) {
        return finish(
            Some(evidence),
            "no_eligible_zone_opens",
            Some(
                "Known native opens remain outside general zone-bridge scope; inspect ordinary/critical routing \
                 failures."
                    .to_owned(),
            ),
        );
    }
    Err(NativeError::Pcb(copper_pcb::Error::NotImplemented(
        "filled-copper bridge trials (filled_feedback.improve_filled_copper)".to_owned(),
    )))
}

//! `pcbir.editor.session`: revision-bound placement previews and reviewed
//! source transactions.
//!
//! The rewrite covers a freshly opened session's scene (`--scene-output`);
//! previews, jobs and source transactions come with the HTTP transport.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use copper_json::{Object, Value};
use copper_lang::compiler::Design;
use copper_lang::mechanical::{MechanicalReference, verify_reference_assets};
use copper_lang::model::Board;
use copper_lang::syntax::{Declaration, MechanicalItem};
use copper_pcb::board::PhysicalBoard;
use copper_physical::{BoardSide, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::features::Scope;
use crate::features::{dimension_source_names, feature_catalog, physical_feature_geometry, source_features};
use crate::overlay::{RoutedOverlay, electrical_digest};
use crate::scene::{RatsnestCache, board_scene, net_costs, ratsnest};
use crate::source::{SourceSnapshot, declaration_spans};
use crate::transactions::{ReviewedSource, electrical_identity};
use crate::{Error, Result, object, sha256_hex, value};

/// `SourceWorkspace`: the compiled source a session may rewrite.
pub struct Workspace {
    pub source: PathBuf,
    pub identity: Board,
    pub input_paths: Vec<PathBuf>,
    pub inputs: Vec<(String, Option<String>)>,
    pub references: Vec<MechanicalReference>,
    /// The physical inputs every rebuild uses (`build`).
    pub settings: crate::project::BuildSettings,
}

fn file_digest(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(sha256_hex(&crate::dependencies::read_source(path)?)))
}

impl Workspace {
    pub fn new(
        source: &Path,
        design: &Design,
        input_paths: Vec<PathBuf>,
        settings: crate::project::BuildSettings,
    ) -> Result<Self> {
        if source.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
            return value("source saving does not follow a symbolic link");
        }
        let input_paths: Vec<PathBuf> = input_paths.iter().map(|p| copper_lang::paths::resolve(p)).collect();
        let mut workspace = Workspace {
            source: source.to_path_buf(),
            identity: electrical_identity(&design.electrical),
            input_paths,
            inputs: Vec::new(),
            references: design.mechanical.as_ref().map(|m| m.references.clone()).unwrap_or_default(),
            settings,
        };
        workspace.inputs = workspace.current_inputs()?;
        Ok(workspace)
    }

    pub(crate) fn current_inputs(&self) -> Result<Vec<(String, Option<String>)>> {
        self.input_paths.iter().map(|p| Ok((copper_lang::paths::display(p), file_digest(p)?))).collect()
    }

    /// `check_inputs`.
    pub fn check_inputs(&self) -> Result<()> {
        verify_reference_assets(&self.references).map_err(Error::Value)?;
        if self.current_inputs()? != self.inputs {
            return value("compiler/footprint inputs changed externally; reload the source workspace");
        }
        Ok(())
    }
}

/// `State`: an immutable board and its temporary session locks.
#[derive(Clone)]
pub struct State {
    pub board: Arc<PhysicalBoard>,
    pub locks: BTreeSet<String>,
    pub manual_preview: bool,
}

impl State {
    fn new(board: Arc<PhysicalBoard>, locks: BTreeSet<String>) -> Self {
        State { board, locks, manual_preview: false }
    }
}

type Pose = (Point, Decimal, BoardSide);

fn poses(board: &PhysicalBoard) -> IndexMap<String, Pose> {
    board.placements.iter().map(|p| (p.reference.clone(), (p.position, p.rotation_degrees.clone(), p.side))).collect()
}

/// `EditorSession`: the accepted state, its preview and its history.
pub struct EditorSession {
    pub source: PathBuf,
    pub source_raw: Vec<u8>,
    pub source_revision: String,
    pub revision: i64,
    pub state: State,
    pub pending: Option<State>,
    undo_stack: Vec<State>,
    redo_stack: Vec<State>,
    source_poses: IndexMap<String, Pose>,
    outputs_stale: bool,
    pub workspace: Option<Workspace>,
    pub overlay: Option<RoutedOverlay>,
    ratsnest_cache: RatsnestCache,
    template: Option<(Arc<PhysicalBoard>, BTreeSet<String>, Object)>,
    job: Option<crate::jobs::PlacementJob>,
    job_consumed: bool,
    source_pending: Option<ReviewedSource>,
    source_undo: Vec<SourceHistory>,
    source_redo: Vec<SourceHistory>,
}

/// One source history entry: the bytes, the session state and the edit's scope.
type SourceHistory = (Vec<u8>, State, Option<Vec<Scope>>);

fn merge(target: &mut Object, entries: Vec<(&'static str, Value)>) {
    for (key, item) in entries {
        target.insert(key.to_owned(), item);
    }
}

impl EditorSession {
    pub fn new(
        board: PhysicalBoard,
        source: &Path,
        workspace: Option<Workspace>,
        overlay: Option<RoutedOverlay>,
    ) -> Result<Self> {
        if !board.tracks.is_empty() || !board.vias.is_empty() || !board.materialized_macros.is_empty() {
            return value("editor placement requires an unrouted physical source");
        }
        let source = copper_lang::paths::resolve(source);
        let source_raw = crate::dependencies::read_source(&source)?;
        let source_revision = sha256_hex(&source_raw);
        let source_poses = poses(&board);
        let board = match &overlay {
            Some(overlay) => {
                let identity = workspace.as_ref().map(|w| electrical_digest(&w.identity));
                overlay.seed(&board, &source_revision, identity.as_deref())
            }
            None => board,
        };
        Ok(EditorSession {
            source,
            source_raw,
            source_revision,
            revision: 0,
            state: State::new(Arc::new(board), BTreeSet::new()),
            pending: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            source_poses,
            outputs_stale: false,
            workspace,
            overlay,
            ratsnest_cache: RatsnestCache::default(),
            template: None,
            job: None,
            job_consumed: false,
            source_pending: None,
            source_undo: Vec::new(),
            source_redo: Vec::new(),
        })
    }

    /// `_poll_job`: a completed job's result becomes the pending preview once.
    fn poll_job(&mut self) -> Result<()> {
        let Some(job) = &mut self.job else { return Ok(()) };
        job.poll()?;
        if job.status != "completed" || self.job_consumed {
            return Ok(());
        }
        self.job_consumed = true;
        let (revision, source_revision) = (job.revision, job.source_revision.clone());
        let checked = self.check(&Value::from(revision)).and_then(|()| {
            if source_revision != self.source_revision {
                return Err(Error::Stale("source changed during placement".to_owned()));
            }
            Ok(())
        });
        let job = self.job.as_mut().expect("job");
        match checked {
            Ok(()) => {
                let board = job.result.take().expect("completed result");
                self.pending = Some(State::new(Arc::new(board), self.state.locks.clone()));
                self.revision += 1;
                Ok(())
            }
            Err(Error::Value(_) | Error::Stale(_) | Error::Os(_)) => {
                job.status = "stale".to_owned();
                job.error = Some("input changed; placement result discarded".to_owned());
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn read_source(&self) -> Result<Vec<u8>> {
        crate::dependencies::read_source(&self.source)
    }

    /// `_document` of a state; the static part is reused while its board and locks are.
    fn document(&mut self, state: &State, pending: bool) -> Result<Object> {
        let reuse = self
            .template
            .as_ref()
            .is_some_and(|(board, locks, _)| Arc::ptr_eq(board, &state.board) && *locks == state.locks);
        if !reuse {
            let mut template = board_scene(
                &state.board,
                &self.source_revision,
                self.revision,
                &state.locks,
                Some(&mut self.ratsnest_cache),
            )?;
            merge(&mut template, physical_feature_geometry(&state.board)?);
            self.template = Some((state.board.clone(), state.locks.clone(), template));
        }
        let mut scene = self.template.as_ref().expect("template").2.clone();
        scene.insert("revision".to_owned(), Value::from(self.revision));
        scene.insert("source_revision".to_owned(), Value::from(self.source_revision.as_str()));
        let board: &PhysicalBoard = &state.board.clone();
        let filename = copper_lang::paths::display(&self.source);
        if let Some(overlay) = &self.overlay {
            let mut identity = self.workspace.as_ref().map(|w| electrical_digest(&w.identity));
            let mut overlay_revision = sha256_hex(&self.read_source()?);
            if let Some(review) = self.reviewing(state) {
                overlay_revision = review.after.revision();
            }
            if let Some(workspace) = &self.workspace
                && let Err(Error::Value(_)) = workspace.check_inputs()
            {
                identity = Some("inputs_changed".to_owned());
            }
            let routed = overlay.scene(board, &overlay_revision, identity.as_deref());
            let stale = routed.get("stale").and_then(|v| match v {
                Value::Bool(b) => Some(*b),
                _ => None,
            }) == Some(true);
            scene.insert("routed_overlay".to_owned(), routed);
            if !stale {
                let evidence = overlay.data.get("evidence");
                let verified = matches!(evidence.and_then(|e| e.get("verified")), Some(Value::Bool(true)));
                let remaining = match evidence.and_then(|e| e.get("remaining")) {
                    Some(Value::Array(items)) => !items.is_empty(),
                    Some(Value::Object(items)) => !items.is_empty(),
                    Some(Value::Int(n)) => *n != 0.into(),
                    Some(Value::Bool(b)) => *b,
                    Some(Value::Str(s)) => !s.is_empty(),
                    Some(Value::Float(f)) => *f != 0.0,
                    Some(Value::Null) | None => false,
                };
                let complete = verified && !remaining;
                let edges = if complete { Vec::new() } else { ratsnest(&overlay.copper_board(board), None)? };
                scene.insert(
                    "connectivity_basis".to_owned(),
                    Value::from(if complete {
                        "native saved-board DRC"
                    } else {
                        "explicit routed copper; planes do not grant credit"
                    }),
                );
                scene.insert("net_costs".to_owned(), net_costs(&edges));
                scene.insert("ratsnest".to_owned(), Value::Array(edges));
            }
        }
        if let Some(workspace) = &self.workspace {
            let flat = copper_lang::elaborate::elaborate(&workspace.identity).map_err(Error::Value)?;
            let nets: BTreeSet<&str> = flat.supplies.iter().map(|s| s.net.as_str()).collect();
            scene.insert("power_nets".to_owned(), Value::Array(nets.into_iter().map(Value::from).collect()));
        }
        scene.insert(
            "scene_profile".to_owned(),
            object(vec![(
                "ratsnest_recomputed_nets",
                Value::Array(self.ratsnest_cache.recomputed_nets.iter().map(|n| Value::from(n.as_str())).collect()),
            )]),
        );
        let writable = self.workspace.is_some();
        scene.insert("source_writable".to_owned(), Value::from(writable));
        scene.insert("placement_unsaved".to_owned(), Value::from(poses(board) != self.source_poses));
        scene.insert("preview_editable".to_owned(), Value::from(pending && state.manual_preview));
        scene.insert("document_kind".to_owned(), Value::from("board"));
        let feature_raw = match self.reviewing(state) {
            Some(review) => review.after.raw.clone(),
            None => self.source_raw.clone(),
        };
        let mut features =
            if writable { source_features(&SourceSnapshot::new(feature_raw, &filename)?)? } else { Vec::new() };
        let snapshot = SourceSnapshot::new(self.source_raw.clone(), &filename)?;
        features.extend(crate::definitions::board_macro_features(board)?);
        if let Some(workspace) = &self.workspace {
            let electrical = match self.reviewing(state) {
                Some(review) => &review.design.electrical,
                None => &workspace.identity,
            };
            let dimensions = dimension_source_names(board, electrical, &features)?;
            if let Some(Value::Array(items)) = scene.get_mut("physical_geometry") {
                for item in items.iter_mut() {
                    let index = item.get("rule_index").and_then(|v| match v {
                        Value::Int(n) => usize::try_from(n).ok(),
                        _ => None,
                    });
                    if let (Some(feature), Value::Object(entry)) = (index.and_then(|i| dimensions.get(&i)), &mut *item)
                    {
                        entry.insert("source_name".to_owned(), feature.get("name").cloned().unwrap_or(Value::Null));
                        entry.insert("targets".to_owned(), feature.get("targets").cloned().unwrap_or(Value::Null));
                    }
                }
            }
        }
        scene.insert("source_features".to_owned(), Value::Array(std::mem::take(&mut features)));
        scene.insert("feature_catalog".to_owned(), Value::Array(if writable { feature_catalog() } else { Vec::new() }));
        if let Some(Value::Object(capabilities)) = scene.get_mut("capabilities") {
            for key in ["source_save", "mechanical_edit", "artwork_edit", "physical_feature_edit"] {
                capabilities.insert(key.to_owned(), Value::from(writable));
            }
        }
        scene.insert("outputs_stale".to_owned(), Value::from(self.outputs_stale));
        scene.insert("can_source_undo".to_owned(), Value::from(!self.source_undo.is_empty()));
        scene.insert("can_source_redo".to_owned(), Value::from(!self.source_redo.is_empty()));
        let review = self.source_pending.as_ref().map(|review| {
            object(vec![
                ("id", Value::from(review.id())),
                ("diff", Value::from(review.diff.as_str())),
                ("summary", Value::from(review.summary.as_str())),
            ])
        });
        scene.insert("source_review".to_owned(), review.unwrap_or(Value::Null));
        if writable {
            scene.insert(
                "notice".to_owned(),
                Value::from(
                    "Source edits require diff review and explicit Save. Placement changes remain temporary unless locked into source. No manufacturing signoff.",
                ),
            );
            let document = copper_lang::parse(&snapshot.text, &filename)?;
            let tokens = copper_lang::tokenize(&snapshot.text, &filename)?;
            let mut mechanical = Vec::new();
            for declaration in &document.declarations {
                let Declaration::Mechanical(block) = declaration else { continue };
                for item in &block.items {
                    let MechanicalItem::Feature(item) = item else { continue };
                    let spans = declaration_spans(&tokens, item.location.offset, false);
                    let parameters: Object =
                        spans.parameters(&snapshot).into_iter().map(|(k, v)| (k, Value::from(v))).collect();
                    mechanical.push(object(vec![
                        ("kind", Value::from(item.kind.as_str())),
                        ("name", Value::from(item.name.as_str())),
                        ("shape", Value::from(item.shape.as_str())),
                        ("line", Value::from(item.location.line as i64)),
                        ("editable", Value::from(true)),
                        ("parameters", Value::Object(parameters)),
                    ]));
                }
            }
            scene.insert("mechanical_features".to_owned(), Value::Array(mechanical));
            if let Some(Value::Array(components)) = scene.get_mut("components") {
                for component in components.iter_mut() {
                    let reference = component.get("reference").and_then(Value::as_str).unwrap_or("").to_owned();
                    let own: Vec<&Vec<String>> = document
                        .declarations
                        .iter()
                        .filter_map(|d| match d {
                            Declaration::Constraint(c)
                                if c.kind == "fixed_placement" && c.targets.contains(&reference) =>
                            {
                                Some(&c.targets)
                            }
                            _ => None,
                        })
                        .collect();
                    let owned = own.len() == 1 && *own[0] == [reference.clone()];
                    if let Value::Object(entry) = component {
                        entry.insert("source_lock_owned".to_owned(), Value::from(owned));
                    }
                }
            }
        }
        if let Some(Value::Array(components)) = scene.get_mut("components") {
            for component in components.iter_mut() {
                if !matches!(component.get("lock_owner"), None | Some(Value::Null)) {
                    continue;
                }
                let reference = component.get("reference").and_then(Value::as_str).unwrap_or("").to_owned();
                let rule = board.placement_rules.iter().find(|r| r.reference == reference);
                if let Value::Object(entry) = component {
                    entry.insert("source_side_locked".to_owned(), Value::from(rule.is_some_and(|r| r.side.is_some())));
                }
            }
        }
        Ok(scene)
    }

    /// `scene`.
    pub fn scene(&mut self) -> Result<Object> {
        self.poll_job()?;
        let state = self.state.clone();
        let mut scene = self.document(&state, false)?;
        scene.insert("can_undo".to_owned(), Value::from(!self.undo_stack.is_empty()));
        scene.insert("can_redo".to_owned(), Value::from(!self.redo_stack.is_empty()));
        let stale = sha256_hex(&self.read_source()?) != self.source_revision;
        scene.insert("source_stale".to_owned(), Value::from(stale));
        let job = match &mut self.job {
            Some(job) => job.poll()?,
            None => Value::Null,
        };
        scene.insert("placement_job".to_owned(), job);
        match verify_reference_assets(&self.state.board.mechanical_references) {
            Ok(()) => {
                scene.insert("references_stale".to_owned(), Value::from(false));
            }
            Err(_) => {
                scene.insert("references_stale".to_owned(), Value::from(true));
                if let Some(Value::Object(warnings)) = scene.get_mut("warnings") {
                    warnings.insert(
                        "reference_assets_changed".to_owned(),
                        Value::from("Locked reference assets changed; reload source before editing. Guides are stale."),
                    );
                }
            }
        }
        let preview = match self.pending.clone() {
            Some(pending) => Value::Object(self.document(&pending, true)?),
            None => Value::Null,
        };
        scene.insert("pending_preview".to_owned(), preview);
        let preview = match &self.source_pending {
            Some(review) => {
                let state = State::new(review.board.clone(), BTreeSet::new());
                Value::Object(self.document(&state, false)?)
            }
            None => Value::Null,
        };
        scene.insert("source_preview".to_owned(), preview);
        Ok(scene)
    }
}

/// Python's `repr` of a JSON value.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(b) => (if *b { "True" } else { "False" }).to_owned(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => copper_json::python_float_repr(*f),
        Value::Str(s) => copper_json::python_str_repr(s),
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", copper_json::python_str_repr(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str(value)` of a JSON value.
fn py_str(value: &Value) -> String {
    match value {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

fn editor<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Value(message.into()))
}

/// `type(value) is int`.
fn exact_int(value: &Value) -> Option<&num_bigint::BigInt> {
    match value {
        Value::Int(n) => Some(n),
        _ => None,
    }
}

/// `_integer`: an integer within +/- 1 km in nanometres.
fn integer(value: &Value, name: &str) -> Result<i64> {
    let bound = num_bigint::BigInt::from(10i64.pow(12));
    match exact_int(value) {
        Some(n) if *n <= bound && *n >= -bound => Ok(i64::try_from(n).expect("bounded")),
        _ => editor(format!("{name} requires an integer within +/- 1 km in nanometres")),
    }
}

fn int_value(value: i128, name: &str) -> Result<i64> {
    if value.abs() > 10i128.pow(12) {
        return editor(format!("{name} requires an integer within +/- 1 km in nanometres"));
    }
    Ok(value as i64)
}

const FIELDS: [(&str, &[&str]); 21] = [
    ("auto_place", &[]),
    ("move", &["reference", "x_nm", "y_nm", "rotation", "side"]),
    ("move_group", &["references", "dx_nm", "dy_nm"]),
    ("lock", &["reference", "locked"]),
    ("apply", &[]),
    ("discard", &[]),
    ("undo", &[]),
    ("redo", &[]),
    ("prepare_lock", &["reference", "x_nm", "y_nm", "rotation", "side", "locks"]),
    ("prepare_placement", &[]),
    ("prepare_mechanical", &["kind", "name", "shape", "parameters", "remove"]),
    ("prepare_mechanical_batch", &["features"]),
    ("review_feature", &["category", "kind", "name", "parameters"]),
    ("review_features", &["features"]),
    ("save_source", &["review_id"]),
    ("discard_source", &[]),
    ("undo_source", &[]),
    ("redo_source", &[]),
    ("reload_source", &[]),
    ("start_auto_place", &[]),
    ("cancel_auto_place", &[]),
];

const SOURCE_ACTIONS: [&str; 10] = [
    "prepare_placement",
    "prepare_lock",
    "prepare_mechanical",
    "prepare_mechanical_batch",
    "review_feature",
    "review_features",
    "save_source",
    "discard_source",
    "undo_source",
    "redo_source",
];

impl EditorSession {
    fn options(&self) -> copper_place::options::PlannerOptions {
        copper_place::options::PlannerOptions {
            candidate_count: 1,
            fixed_references: self.state.locks.iter().cloned().collect(),
            ..copper_place::options::PlannerOptions::default()
        }
    }

    /// `_check`: the request's revision, the source bytes and the inputs are current.
    fn check(&self, revision: &Value) -> Result<()> {
        if exact_int(revision) != Some(&num_bigint::BigInt::from(self.revision)) {
            return Err(Error::Stale("session changed; reload the current scene".to_owned()));
        }
        if sha256_hex(&self.read_source()?) != self.source_revision {
            return Err(Error::Stale(
                "source changed externally; reload the source before applying previews".to_owned(),
            ));
        }
        if let Some(workspace) = &self.workspace {
            workspace.check_inputs()?;
        }
        verify_reference_assets(&self.state.board.mechanical_references).map_err(Error::Value)
    }

    /// `_commit`.
    fn commit(&mut self, state: State) {
        // Session states only ever differ from each other in poses and metadata.
        let same = Arc::ptr_eq(&state.board, &self.state.board)
            || (state.board.placements == self.state.board.placements
                && state.board.metadata == self.state.board.metadata);
        if !same {
            self.outputs_stale = true;
        }
        self.undo_stack.push(self.state.clone());
        if self.undo_stack.len() > 64 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.state = State { manual_preview: false, ..state };
        self.pending = None;
        self.source_pending = None;
        self.revision += 1;
    }

    /// `_check_pose_ownership`: invalid geometry never changes fixed intent.
    fn check_pose_ownership(
        &self,
        before: &IndexMap<String, copper_pcb::board::Placement>,
        after: &IndexMap<String, copper_pcb::board::Placement>,
    ) -> Result<()> {
        for reference in &self.state.locks {
            if after.get(reference) != before.get(reference) {
                return editor(format!("unlock the temporary pose before moving {reference}"));
            }
        }
        for rule in &self.state.board.placement_rules {
            let pose = &after[&rule.reference];
            if rule.fixed_position.is_some_and(|p| p != pose.position)
                || rule.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != pose.rotation_degrees)
                || rule.side.is_some_and(|s| s != pose.side)
            {
                return editor(format!(
                    "{}: pose violates source-fixed position, rotation or side; edit its source locks explicitly",
                    rule.reference
                ));
            }
        }
        Ok(())
    }

    /// A manual preview of `moved` on the accepted board.
    fn manual_preview(&mut self, moved: &IndexMap<String, copper_pcb::board::Placement>) -> Result<Object> {
        let mut trial = (*self.state.board).clone();
        trial.placements = self.state.board.placements.iter().map(|p| moved[&p.reference].clone()).collect();
        trial.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
        trial.metadata.insert("editor_preview".to_owned(), "true".to_owned());
        let pending = State { board: Arc::new(trial), locks: self.state.locks.clone(), manual_preview: true };
        self.pending = Some(pending.clone());
        self.revision += 1;
        let preview = self.document(&pending, true)?;
        Ok(response(vec![("preview", Value::Object(preview)), ("accepted", Value::from(false))]))
    }

    fn accepted(&mut self) -> Result<Object> {
        Ok(response(vec![("scene", Value::Object(self.scene()?)), ("accepted", Value::from(true))]))
    }

    /// `operation`: one revision-bound request.
    pub fn operation(&mut self, request: &Object) -> Result<Object> {
        let action = request.get("action").and_then(Value::as_str).unwrap_or("");
        let Some((_, fields)) = FIELDS.iter().find(|(name, _)| *name == action) else {
            return editor("unknown editor operation");
        };
        let mut required: Vec<&str> = vec!["action", "revision"];
        required.extend(fields.iter());
        let optional: &[&str] = if action == "review_feature" { &["targets", "remove"] } else { &[] };
        if !required.iter().all(|k| request.contains_key(*k))
            || request.keys().any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
        {
            return editor("unexpected or missing operation fields");
        }
        let revision = &request["revision"];
        if action == "cancel_auto_place" {
            if exact_int(revision) != Some(&num_bigint::BigInt::from(self.revision)) {
                return Err(Error::Stale("session changed; reload the scene".to_owned()));
            }
            if let Some(job) = &mut self.job {
                job.cancel("cancelled", None);
            }
            return self.accepted();
        }
        if action == "reload_source" {
            if exact_int(revision) != Some(&num_bigint::BigInt::from(self.revision)) {
                return Err(Error::Stale("session changed; reload the current scene".to_owned()));
            }
            return self.reload_source();
        }
        self.check(revision)?;
        if let Some(job) = &mut self.job
            && job.poll()?.get("status").and_then(Value::as_str) == Some("running")
        {
            job.cancel("stale", Some("session edited; old placement cancelled"));
        }
        if self.source_pending.is_some() && action != "save_source" && action != "discard_source" {
            return editor("save or discard the source review before another operation");
        }
        if SOURCE_ACTIONS.contains(&action) {
            return self.source_operation(action, request);
        }
        match action {
            "start_auto_place" => {
                self.pending = None;
                self.revision += 1;
                self.job_consumed = false;
                let job = crate::jobs::PlacementJob::new(
                    &self.state.board,
                    self.options(),
                    self.revision,
                    &self.source_revision,
                )?;
                self.job = Some(job);
                self.accepted()
            }
            "auto_place" => {
                self.pending = None;
                let planned = copper_place::layout::plan_placement(&self.state.board, &self.options())?.board;
                let pending = State::new(Arc::new(planned), self.state.locks.clone());
                self.pending = Some(pending.clone());
                self.revision += 1;
                let preview = self.document(&pending, true)?;
                Ok(response(vec![("preview", Value::Object(preview)), ("accepted", Value::from(false))]))
            }
            "move" => self.move_one(request),
            "move_group" => self.move_group(request),
            "lock" => {
                let (Value::Bool(locked), Value::Str(reference)) = (&request["locked"], &request["reference"]) else {
                    return editor("lock requires a component reference and boolean locked");
                };
                if !self.state.board.placements.iter().any(|p| p.reference == *reference) {
                    return editor("unknown component reference");
                }
                let mut unit: BTreeSet<String> = BTreeSet::from([reference.clone()]);
                for cluster in &self.state.board.rigid_clusters {
                    if cluster.members.iter().any(|m| m.reference == *reference) {
                        unit.extend(cluster.members.iter().map(|m| m.reference.clone()));
                    }
                }
                let locks: BTreeSet<String> = if *locked {
                    self.state.locks.union(&unit).cloned().collect()
                } else {
                    self.state.locks.difference(&unit).cloned().collect()
                };
                self.commit(State::new(self.state.board.clone(), locks));
                self.accepted()
            }
            "apply" => {
                let Some(pending) = self.pending.clone() else { return editor("no pending preview") };
                let placements = copper_pcb::keepouts::placement_map(&pending.board);
                let policy = copper_drc::legality::Policy {
                    fixed_references: self.state.locks.iter().cloned().collect(),
                    ..copper_drc::legality::Policy::default()
                };
                let checker = copper_drc::legality::Checker::new(&self.state.board, policy);
                if !checker.placement_solution_is_hard_legal(&placements)? {
                    return editor("pending placement no longer satisfies hard physical rules");
                }
                self.commit(pending);
                self.accepted()
            }
            "discard" => {
                self.pending = None;
                self.revision += 1;
                self.accepted()
            }
            _ => {
                let undo = action == "undo";
                let taken = if undo { self.undo_stack.pop() } else { self.redo_stack.pop() };
                let Some(taken) = taken else { return editor(format!("nothing to {action}")) };
                let current = std::mem::replace(&mut self.state, taken);
                if undo {
                    self.redo_stack.push(current)
                } else {
                    self.undo_stack.push(current)
                }
                self.pending = None;
                self.revision += 1;
                self.accepted()
            }
        }
    }

    fn base(&self) -> State {
        match &self.pending {
            Some(pending) if pending.manual_preview => pending.clone(),
            _ => self.state.clone(),
        }
    }

    fn move_one(&mut self, request: &Object) -> Result<Object> {
        let base = self.base();
        let poses = copper_pcb::keepouts::placement_map(&base.board);
        let reference = match &request["reference"] {
            Value::Str(reference) if poses.contains_key(reference) => reference.clone(),
            _ => return editor("unknown component reference"),
        };
        if self.state.locks.contains(&reference) {
            return editor("unlock the temporary pose before moving it");
        }
        let text = py_str(&request["rotation"]);
        let rotation = match Decimal::parse(&text) {
            Ok(rotation) => Some(rotation),
            Err(copper_units::DecimalError::Syntax(_)) => {
                return editor("[<class 'decimal.ConversionSyntax'>]");
            }
            Err(_) => None,
        };
        let limit = Decimal::from_i64(3600);
        let Some(rotation) = rotation.filter(|r| r.abs() <= limit) else {
            return editor("rotation must be finite degrees within +/- 3600");
        };
        let x = integer(&request["x_nm"], "x_nm")?;
        let y = integer(&request["y_nm"], "y_nm")?;
        let side = match &request["side"] {
            Value::Str(side) if side == "front" => BoardSide::Front,
            Value::Str(side) if side == "back" => BoardSide::Back,
            other => return editor(format!("{} is not a valid BoardSide", py_repr(other))),
        };
        let mut proposed = poses[&reference].clone();
        proposed.position = Point::new(x, y);
        proposed.rotation_degrees = copper_pcb::decimal_rem(&rotation, 360).map_err(|e| Error::Value(e.to_string()))?;
        proposed.side = side;
        let moved = move_placement_unit(&base.board, &poses, &reference, proposed)?;
        self.check_pose_ownership(&poses, &moved)?;
        self.manual_preview(&moved)
    }

    fn move_group(&mut self, request: &Object) -> Result<Object> {
        let base = self.base();
        let poses = copper_pcb::keepouts::placement_map(&base.board);
        let references: Vec<String> = match &request["references"] {
            Value::Array(items)
                if !items.is_empty()
                    && items.len() <= poses.len()
                    && items.iter().all(|i| matches!(i, Value::Str(s) if !s.is_empty())) =>
            {
                items.iter().filter_map(Value::as_str).map(str::to_owned).collect()
            }
            _ => return editor("group move requires a non-empty list of unique component references"),
        };
        let unique: BTreeSet<&String> = references.iter().collect();
        if unique.len() != references.len() {
            return editor("group move requires a non-empty list of unique component references");
        }
        let unknown: BTreeSet<&String> = references.iter().filter(|r| !poses.contains_key(*r)).collect();
        if !unknown.is_empty() {
            let names: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
            return editor(format!("unknown component reference(s): {}", names.join(", ")));
        }
        let dx = integer(&request["dx_nm"], "dx_nm")?;
        let dy = integer(&request["dy_nm"], "dy_nm")?;
        let mut members: BTreeSet<String> = references.iter().cloned().collect();
        for cluster in &self.state.board.rigid_clusters {
            let cluster_references: BTreeSet<String> = cluster.members.iter().map(|m| m.reference.clone()).collect();
            if !members.is_disjoint(&cluster_references) {
                members.extend(cluster_references);
            }
        }
        let locked: Vec<&str> = members.iter().filter(|m| self.state.locks.contains(*m)).map(String::as_str).collect();
        if !locked.is_empty() {
            return editor(format!("unlock the temporary pose before moving the group: {}", locked.join(", ")));
        }
        let fixed: Vec<&str> = members
            .iter()
            .filter(|m| {
                self.state.board.placement_rules.iter().any(|r| r.fixed_position.is_some() && r.reference == **m)
            })
            .map(String::as_str)
            .collect();
        if !fixed.is_empty() {
            return editor(format!("group contains source-position-locked component(s): {}", fixed.join(", ")));
        }
        let mut moved = poses.clone();
        for reference in &members {
            let pose = &poses[reference];
            let mut next = pose.clone();
            next.position = Point::new(
                int_value(i128::from(pose.position.x_nm) + i128::from(dx), "x_nm")?,
                int_value(i128::from(pose.position.y_nm) + i128::from(dy), "y_nm")?,
            );
            moved.insert(reference.clone(), next);
        }
        self.check_pose_ownership(&poses, &moved)?;
        self.manual_preview(&moved)
    }
}

fn response(entries: Vec<(&str, Value)>) -> Object {
    entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// `move_placement_unit`: a member's proposed pose moves its whole rigid unit.
fn move_placement_unit(
    board: &PhysicalBoard,
    placements: &IndexMap<String, copper_pcb::board::Placement>,
    reference: &str,
    proposed: copper_pcb::board::Placement,
) -> Result<IndexMap<String, copper_pcb::board::Placement>> {
    let mut result = placements.clone();
    let Some(cluster) = board.rigid_clusters.iter().find(|c| c.members.iter().any(|m| m.reference == reference)) else {
        result.insert(reference.to_owned(), proposed);
        return Ok(result);
    };
    let local = cluster.members.iter().find(|m| m.reference == reference).expect("member");
    let anchor_member = cluster.members.iter().find(|m| m.reference == cluster.anchor_reference).expect("anchor");
    let full = Decimal::from_i64(360);
    let rotation = proposed
        .rotation_degrees
        .sub(&local.rotation_degrees)
        .and_then(|r| r.rem(&full))
        .and_then(|r| r.add(&full))
        .and_then(|r| r.rem(&full))
        .map_err(|e| Error::Value(e.to_string()))?;
    let mut frame = proposed.clone();
    frame.rotation_degrees = rotation.clone();
    let mut anchor = placements[&cluster.anchor_reference].clone();
    anchor.side = proposed.side;
    anchor.rotation_degrees = rotation;
    anchor.position = copper_pcb::transform::transformed_local_point(
        &frame,
        Point::new(
            anchor_member.position.x_nm - local.position.x_nm,
            anchor_member.position.y_nm - local.position.y_nm,
        ),
    );
    result.extend(copper_pcb::hard_macros::cluster_placements(cluster, &anchor, placements)?);
    Ok(result)
}

fn metadata_object(board: &PhysicalBoard, key: &str) -> Result<Object> {
    let text = board.metadata.get(key).map_or("{}", String::as_str);
    match copper_json::parse(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Ok(Object::new()),
        Err(error) => Err(Error::Uncaught(format!("JSONDecodeError: {error}"))),
    }
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(v.as_str())).collect())
}

/// The fixed-placement targets an imported module (not this source) owns.
fn imported_fixed_targets(design: &Design, source: &Path) -> Result<Vec<(Vec<String>, Vec<String>)>> {
    let prefix = format!("{}:", copper_lang::paths::display(source));
    let flat = copper_lang::elaborate::elaborate(&design.electrical).map_err(Error::Value)?;
    Ok(flat
        .constraints
        .iter()
        .filter(|c| {
            c.kind == copper_lang::model::ConstraintKind::FixedPlacement
                && c.origins.iter().any(|origin| !origin.starts_with(&prefix))
        })
        .map(|c| (c.targets.clone(), c.origins.clone()))
        .collect())
}

impl EditorSession {
    /// Whether `state` shows the pending source review's board.
    fn reviewing(&self, state: &State) -> Option<&ReviewedSource> {
        self.source_pending.as_ref().filter(|review| Arc::ptr_eq(&review.board, &state.board))
    }

    fn workspace(&self) -> Result<&Workspace> {
        match &self.workspace {
            Some(workspace) => Ok(workspace),
            None => editor("source workspace is read-only"),
        }
    }

    fn compile(&self, raw: &[u8]) -> Result<Design> {
        let filename = copper_lang::paths::display(&self.source);
        let snapshot = SourceSnapshot::new(raw.to_vec(), &filename)?;
        Ok(copper_lang::compiler::compile_design_source(&snapshot.text, &filename, true, true)?)
    }

    fn build(&self, design: &Design) -> Result<PhysicalBoard> {
        crate::project::build(design, &self.source, &self.workspace()?.settings)
    }

    /// `_source_operation`.
    fn source_operation(&mut self, action: &str, request: &Object) -> Result<Object> {
        let workspace = self.workspace()?;
        if self.pending.is_some() && !["prepare_placement", "save_source", "discard_source"].contains(&action) {
            return editor("apply or discard the placement preview before editing source");
        }
        let snapshot = SourceSnapshot::new(self.read_source()?, &copper_lang::paths::display(&self.source))?;
        let seeds = self.state.board.clone();
        match action {
            "prepare_placement" => self.review_placement(&snapshot)?,
            "review_features" => {
                let (raw, scope) = crate::features::feature_batch(&snapshot, &request["features"])?;
                self.source_pending = Some(workspace.review(&snapshot, raw, &seeds, scope)?);
            }
            "review_feature" => {
                let fields: Object = request
                    .iter()
                    .filter(|(k, _)| *k != "action" && *k != "revision")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let (patch, scope) = crate::features::feature_patch(&snapshot, &fields)?;
                let raw = patch.apply(&snapshot)?;
                self.source_pending = Some(workspace.review(&snapshot, raw, &seeds, scope.map(|s| vec![s]))?);
            }
            "prepare_lock" => {
                let raw = self.prepare_lock(&snapshot, request)?;
                let review = self.workspace()?.review(&snapshot, raw, &seeds, None)?;
                self.source_pending = Some(review);
            }
            "prepare_mechanical" | "prepare_mechanical_batch" => {
                let raw = self.prepare_mechanical(&snapshot, action, request)?;
                let review = self.workspace()?.review(&snapshot, raw, &seeds, None)?;
                self.source_pending = Some(review);
            }
            "save_source" => {
                let review = match &self.source_pending {
                    Some(review) if request["review_id"].as_str() == Some(review.id().as_str()) => review.clone(),
                    _ => return Err(Error::Stale("review changed; inspect the current source diff".to_owned())),
                };
                let (_, validated) = workspace.validate(&review.after.raw, &review.board, review.scope.as_deref())?;
                if validated != *review.board {
                    return editor("physical inputs changed since review; discard and prepare the edit again");
                }
                workspace.save(&review)?;
                self.source_undo.push((review.before.raw.clone(), self.state.clone(), review.scope.clone()));
                if self.source_undo.len() > 64 {
                    self.source_undo.remove(0);
                }
                self.source_redo.clear();
                self.saved(review.after.raw.clone(), &review.board)?;
                return self.accepted();
            }
            "discard_source" => self.source_pending = None,
            _ => {
                let undo = action == "undo_source";
                let take = if undo { &self.source_undo } else { &self.source_redo };
                let Some((raw, state, scope)) = take.last().cloned() else {
                    return editor("no source history available");
                };
                let review = workspace.review(&snapshot, raw.clone(), &state.board, scope.clone())?;
                workspace.save(&review)?;
                let entry = (snapshot.raw.clone(), self.state.clone(), scope);
                if undo {
                    self.source_undo.pop();
                    self.source_redo.push(entry);
                } else {
                    self.source_redo.pop();
                    self.source_undo.push(entry);
                }
                self.saved(raw, &review.board)?;
                return self.accepted();
            }
        }
        self.revision += 1;
        let scene = self.scene()?;
        let preview = if self.source_pending.is_some() { scene.get("source_preview").cloned() } else { None };
        let mut entries = vec![("scene", Value::Object(scene)), ("accepted", Value::from(true))];
        if let Some(preview) = preview {
            entries.push(("source_preview", preview));
        }
        Ok(response(entries))
    }

    /// The candidate bytes of `prepare_lock`.
    fn prepare_lock(&self, snapshot: &SourceSnapshot, request: &Object) -> Result<Vec<u8>> {
        let board = &self.state.board;
        let references: BTreeSet<String> = board.placements.iter().map(|p| p.reference.clone()).collect();
        let reference = match &request["reference"] {
            Value::Str(reference) if references.contains(reference) => reference.clone(),
            _ => return editor("unknown component reference"),
        };
        let owners = metadata_object(board, "mechanical_attachment_owners")?;
        if let Some(owner) = owners.get(&reference).filter(|owner| truthy(owner)) {
            return editor(format!(
                "pose is owned by mechanical attachment {}; edit that attachment instead",
                py_repr(owner)
            ));
        }
        let current = self.compile(&snapshot.raw)?;
        if imported_fixed_targets(&current, &self.source)?.iter().any(|(targets, _)| targets.contains(&reference)) {
            return editor("fixed pose is owned by an imported module; edit its public mechanical binding instead");
        }
        let locks: Vec<&str> = match &request["locks"] {
            Value::Array(items) if items.iter().all(|k| matches!(k, Value::Str(_))) => {
                items.iter().map(|k| k.as_str().expect("str")).collect()
            }
            _ => return editor("locks must be unique position/rotation/side names"),
        };
        let unique: BTreeSet<&str> = locks.iter().copied().collect();
        if unique.len() != locks.len() || unique.iter().any(|k| !["position", "rotation", "side"].contains(k)) {
            return editor("locks must be unique position/rotation/side names");
        }
        let given = |key: &str| Some(request[key].clone()).filter(|v| !matches!(v, Value::Null));
        let mut edit = crate::source::PlacementEdit::default();
        if unique.contains("position") {
            edit.x_nm = given("x_nm");
            edit.y_nm = given("y_nm");
        } else {
            edit.clear.extend(["x", "y"]);
        }
        if unique.contains("rotation") {
            edit.rotation = given("rotation").map(crate::source::Rotation::Json);
        } else {
            edit.clear.push("rotation");
        }
        if unique.contains("side") {
            edit.side = given("side");
        } else {
            edit.clear.push("side");
        }
        crate::source::fixed_placement_patch(snapshot, &reference, &edit, &references)?.apply(snapshot)
    }

    /// The candidate bytes of `prepare_mechanical` and `prepare_mechanical_batch`.
    fn prepare_mechanical(&self, snapshot: &SourceSnapshot, action: &str, request: &Object) -> Result<Vec<u8>> {
        let keys = ["kind", "name", "shape", "parameters", "remove"];
        let features: Vec<Value> = if action == "prepare_mechanical_batch" {
            match &request["features"] {
                Value::Array(items) if (1..=513).contains(&items.len()) => items.clone(),
                _ => return editor("mechanical batch requires 1–513 feature edits"),
            }
        } else {
            vec![Value::Object(keys.iter().map(|k| ((*k).to_owned(), request[*k].clone())).collect())]
        };
        let provenance = metadata_object(&self.state.board, "mechanical_provenance")?;
        let provenance: Vec<&Value> = match provenance.get("features") {
            Some(Value::Array(items)) => items.iter().collect(),
            _ => Vec::new(),
        };
        let mut current = snapshot.clone();
        let mut seen: Vec<(String, String)> = Vec::new();
        for feature in &features {
            let valid = match feature {
                Value::Object(map) => {
                    map.len() == keys.len()
                        && keys.iter().all(|k| map.contains_key(*k))
                        && matches!(&map["parameters"], Value::Object(p) if p.len() <= 32 && p.values().all(|v| matches!(v, Value::Str(_))))
                        && ["kind", "name", "shape"].iter().all(|k| matches!(map[*k], Value::Str(_)))
                        && matches!(map["remove"], Value::Bool(_))
                }
                _ => false,
            };
            if !valid {
                return editor("mechanical feature requires a bounded literal property map");
            }
            let feature = feature.as_object().expect("object");
            let text = |k: &str| feature[k].as_str().expect("str").to_owned();
            let (kind, name, shape) = (text("kind"), text("name"), text("shape"));
            let identity = (kind.clone(), name.clone());
            if seen.contains(&identity) {
                return editor("mechanical batch cannot edit a feature twice");
            }
            seen.push(identity);
            let imported = provenance.iter().any(|f| {
                f.get("kind").and_then(Value::as_str) == Some(kind.as_str())
                    && f.get("name").and_then(Value::as_str) == Some(name.as_str())
                    && f.get("profile").is_some_and(truthy)
            });
            if imported {
                return editor("mechanical feature is owned by an imported profile");
            }
            let parameters: Vec<(String, String)> = feature["parameters"]
                .as_object()
                .expect("object")
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().expect("str").to_owned()))
                .collect();
            let remove = matches!(feature["remove"], Value::Bool(true));
            let patch = crate::source::mechanical_patch(&current, &kind, &name, &shape, &parameters, remove)?;
            current = SourceSnapshot::new(patch.apply(&current)?, &snapshot.filename)?;
        }
        Ok(current.raw)
    }

    /// `_review_placement`: freeze the reviewed layout as source locks.
    fn review_placement(&mut self, snapshot: &SourceSnapshot) -> Result<()> {
        let board = self.pending.as_ref().unwrap_or(&self.state).board.clone();
        let current_poses = poses(&board);
        if current_poses == self.source_poses {
            return editor("no unsaved placement changes");
        }
        let placements = copper_pcb::keepouts::placement_map(&board);
        if !copper_drc::legality::placement_solution_is_legal(&board, &placements)? {
            let reasons = copper_drc::legality::placement_rejection_reasons(&board, &placements)?;
            return editor(format!("placement cannot be saved: {}", reasons.join("; ")));
        }
        let design = self.compile(&snapshot.raw)?;
        let imported: BTreeSet<String> =
            imported_fixed_targets(&design, &self.source)?.into_iter().flat_map(|(targets, _)| targets).collect();
        let attachments = metadata_object(&board, "mechanical_attachment_owners")?;
        let members: BTreeSet<&str> =
            board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.as_str())).collect();
        let anchors: BTreeSet<&str> = board.rigid_clusters.iter().map(|c| c.anchor_reference.as_str()).collect();
        let references: BTreeSet<String> = current_poses.keys().cloned().collect();
        let mut current = snapshot.clone();
        let mut saved = 0;
        for pose in &board.placements {
            let reference = pose.reference.as_str();
            if members.contains(reference) && !anchors.contains(reference) {
                continue;
            }
            let rule = board.placement_rules.iter().find(|r| r.reference == reference);
            if rule
                .is_some_and(|r| r.fixed_position.is_some() && r.fixed_rotation_degrees.is_some() && r.side.is_some())
            {
                continue;
            }
            if imported.contains(reference) || attachments.contains_key(reference) {
                return editor(format!(
                    "{reference}: pose is owned by an imported module or mechanical attachment; edit its binding before saving placement"
                ));
            }
            let edit = crate::source::PlacementEdit {
                x_nm: Some(Value::from(pose.position.x_nm)),
                y_nm: Some(Value::from(pose.position.y_nm)),
                rotation: Some(crate::source::Rotation::Decimal(pose.rotation_degrees.clone())),
                side: Some(Value::from(pose.side.value())),
                clear: Vec::new(),
            };
            let patch = crate::source::fixed_placement_patch(&current, reference, &edit, &references)?;
            current = SourceSnapshot::new(patch.apply(&current)?, &snapshot.filename)?;
            saved += 1;
        }
        let mut review = self.workspace()?.review(snapshot, current.raw, &board, None)?;
        if poses(&self.build(&review.design)?) != current_poses {
            return editor("saved source does not reproduce this layout; resolve placement ownership before saving");
        }
        review.summary = format!(
            "Save layout as position, rotation and side locks for {saved} placement units. To move them later, edit their Saved source locks. Routing and manufacturing outputs must be rebuilt."
        );
        self.source_pending = Some(review);
        Ok(())
    }

    /// `_saved`: the session continues from the saved source.
    fn saved(&mut self, raw: Vec<u8>, board: &PhysicalBoard) -> Result<()> {
        let design = self.compile(&raw)?;
        let source_poses = poses(&self.build(&design)?);
        let workspace = self.workspace.as_mut().expect("workspace");
        workspace.identity = electrical_identity(&design.electrical);
        self.source_poses = source_poses;
        self.source_revision = sha256_hex(&raw);
        self.source_raw = raw;
        let mut board = board.clone();
        board.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
        board.metadata.insert("editor_outputs_stale".to_owned(), "true".to_owned());
        workspace.references = board.mechanical_references.clone();
        self.state = State::new(Arc::new(board), BTreeSet::new());
        self.pending = None;
        self.source_pending = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.outputs_stale = true;
        self.revision += 1;
        Ok(())
    }

    /// `reload_source`: discard previews and history and continue from the source on disk.
    fn reload_source(&mut self) -> Result<Object> {
        self.workspace()?;
        let draft = self.pending.as_ref().unwrap_or(&self.state).board.clone();
        let draft_poses = poses(&draft);
        let moved: BTreeSet<String> = draft_poses
            .iter()
            .filter(|(reference, pose)| self.source_poses.get(*reference) != Some(pose))
            .map(|(reference, _)| reference.clone())
            .collect();
        let raw = self.read_source()?;
        let design = self.compile(&raw)?;
        let diagnostics = copper_lang::erc::check(&design.electrical, false).map_err(Error::Value)?;
        if copper_lang::erc::has_errors(&diagnostics) {
            return editor("external source fails ERC");
        }
        let source_board = self.build(&design)?;
        let board = if moved.is_empty() {
            source_board.clone()
        } else {
            crate::transactions::seed_placements(source_board.clone(), &draft, Some(&moved))?
        };
        let reloaded = poses(&board);
        let kept: Vec<String> = moved.iter().filter(|r| reloaded.get(*r) == draft_poses.get(*r)).cloned().collect();
        let dropped: Vec<String> = moved.iter().filter(|r| !kept.contains(r)).cloned().collect();
        let mut invalid = Vec::new();
        if !kept.is_empty() {
            let placements = copper_pcb::keepouts::placement_map(&board);
            let reasons = copper_drc::legality::placement_rejection_reasons(&board, &placements)?;
            let named: BTreeSet<&str> =
                reasons.iter().flat_map(|message| message.split(':').next().unwrap_or("").split(" / ")).collect();
            invalid = kept.iter().filter(|r| named.contains(r.as_str())).cloned().collect();
        }
        let workspace = self.workspace.as_mut().expect("workspace");
        workspace.identity = electrical_identity(&design.electrical);
        workspace.inputs = workspace.current_inputs()?;
        workspace.references = board.mechanical_references.clone();
        self.source_revision = sha256_hex(&raw);
        self.source_raw = raw;
        self.state = State::new(Arc::new(board), BTreeSet::new());
        self.source_poses = poses(&source_board);
        self.pending = None;
        self.source_pending = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.source_undo.clear();
        self.source_redo.clear();
        self.outputs_stale = true;
        if let Some(job) = &mut self.job {
            job.cancel("cancelled", None);
        }
        self.revision += 1;
        let scene = self.scene()?;
        Ok(response(vec![
            ("scene", Value::Object(scene)),
            ("accepted", Value::from(true)),
            ("placement_kept", strings(&kept)),
            ("placement_dropped", strings(&dropped)),
            ("placement_invalid", strings(&invalid)),
        ]))
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(n) => *n != 0.into(),
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

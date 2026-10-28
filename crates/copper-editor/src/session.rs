//! `pcbir.editor.session`: revision-bound placement previews and reviewed
//! source transactions.
//!
//! The rewrite covers a freshly opened session's scene (`--scene-output`);
//! previews, jobs and source transactions come with the HTTP transport.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use copper_json::{Object, Value};
use copper_lang::compiler::Design;
use copper_lang::mechanical::{MechanicalReference, verify_reference_assets};
use copper_lang::model::Board;
use copper_lang::syntax::{Declaration, MechanicalItem};
use copper_pcb::board::PhysicalBoard;

use crate::features::{dimension_source_names, feature_catalog, physical_feature_geometry, source_features};
use crate::overlay::{RoutedOverlay, electrical_digest};
use crate::scene::{RatsnestCache, board_scene, net_costs, ratsnest};
use crate::source::{SourceSnapshot, declaration_spans};
use crate::transactions::electrical_identity;
use crate::{Error, Result, object, sha256_hex, value};

/// `SourceWorkspace`: the compiled source a session may rewrite.
pub struct Workspace {
    pub source: PathBuf,
    pub identity: Board,
    pub input_paths: Vec<PathBuf>,
    pub inputs: Vec<(String, Option<String>)>,
    pub references: Vec<MechanicalReference>,
}

fn file_digest(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(sha256_hex(&crate::dependencies::read_source(path)?)))
}

impl Workspace {
    pub fn new(source: &Path, design: &Design, input_paths: Vec<PathBuf>) -> Result<Self> {
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
        };
        workspace.inputs = workspace.current_inputs()?;
        Ok(workspace)
    }

    fn current_inputs(&self) -> Result<Vec<(String, Option<String>)>> {
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

/// `EditorSession` as freshly opened.
pub struct EditorSession {
    pub board: PhysicalBoard,
    pub source: PathBuf,
    pub source_raw: Vec<u8>,
    pub source_revision: String,
    pub revision: i64,
    pub locks: BTreeSet<String>,
    pub workspace: Option<Workspace>,
    pub overlay: Option<RoutedOverlay>,
    ratsnest_cache: RatsnestCache,
}

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
        let board = match &overlay {
            Some(overlay) => {
                let identity = workspace.as_ref().map(|w| electrical_digest(&w.identity));
                overlay.seed(&board, &source_revision, identity.as_deref())
            }
            None => board,
        };
        Ok(EditorSession {
            board,
            source,
            source_raw,
            source_revision,
            revision: 0,
            locks: BTreeSet::new(),
            workspace,
            overlay,
            ratsnest_cache: RatsnestCache::default(),
        })
    }

    fn read_source(&self) -> Result<Vec<u8>> {
        crate::dependencies::read_source(&self.source)
    }

    /// `_document` of the current state.
    fn document(&mut self) -> Result<Object> {
        let board = self.board.clone();
        let mut scene =
            board_scene(&board, &self.source_revision, self.revision, &self.locks, Some(&mut self.ratsnest_cache))?;
        merge(&mut scene, physical_feature_geometry(&board)?);
        let filename = copper_lang::paths::display(&self.source);
        if let Some(overlay) = &self.overlay {
            let mut identity = self.workspace.as_ref().map(|w| electrical_digest(&w.identity));
            let overlay_revision = sha256_hex(&self.read_source()?);
            if let Some(workspace) = &self.workspace
                && let Err(Error::Value(_)) = workspace.check_inputs()
            {
                identity = Some("inputs_changed".to_owned());
            }
            let routed = overlay.scene(&board, &overlay_revision, identity.as_deref());
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
                let edges = if complete { Vec::new() } else { ratsnest(&overlay.copper_board(&board), None)? };
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
        scene.insert("placement_unsaved".to_owned(), Value::from(false));
        scene.insert("preview_editable".to_owned(), Value::from(false));
        scene.insert("document_kind".to_owned(), Value::from("board"));
        let snapshot = SourceSnapshot::new(self.source_raw.clone(), &filename)?;
        let mut features = if writable { source_features(&snapshot)? } else { Vec::new() };
        features.extend(crate::definitions::board_macro_features(&board)?);
        if let Some(workspace) = &self.workspace {
            let dimensions = dimension_source_names(&board, &workspace.identity, &features)?;
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
        scene.insert("outputs_stale".to_owned(), Value::from(false));
        scene.insert("can_source_undo".to_owned(), Value::from(false));
        scene.insert("can_source_redo".to_owned(), Value::from(false));
        scene.insert("source_review".to_owned(), Value::Null);
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
        let mut scene = self.document()?;
        scene.insert("can_undo".to_owned(), Value::from(false));
        scene.insert("can_redo".to_owned(), Value::from(false));
        let stale = sha256_hex(&self.read_source()?) != self.source_revision;
        scene.insert("source_stale".to_owned(), Value::from(stale));
        scene.insert("placement_job".to_owned(), Value::Null);
        match verify_reference_assets(&self.board.mechanical_references) {
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
        scene.insert("pending_preview".to_owned(), Value::Null);
        scene.insert("source_preview".to_owned(), Value::Null);
        Ok(scene)
    }
}

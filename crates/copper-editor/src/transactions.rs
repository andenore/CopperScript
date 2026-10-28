//! `pcbir.editor.transactions`: the electrical identity an edit must keep,
//! session seeds, and the reviewed source review/save transaction.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use copper_drc::legality;
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::mechanical::verify_reference_assets;
use copper_lang::model::{Board, Constraint, ConstraintKind, MacroBinding};
use copper_lang::syntax::Location;
use copper_pcb::board::{PhysicalBoard, Placement};
use copper_pcb::keepouts::placement_map;
use indexmap::IndexMap;

use crate::difflib::{splitlines_keepends, unified_diff};
use crate::features::Scope;
use crate::project::{apply_fixed_cluster_poses, build};
use crate::server::random_token;
use crate::session::Workspace;
use crate::source::SourceSnapshot;
use crate::{Error, Result, sha256_hex, value};

fn neutral(location: &Location) -> Location {
    Location { filename: location.filename.clone(), offset: 0, line: 0, column: 0 }
}

/// `macro_binding_identity`: ignore only diagnostic offsets.
pub fn macro_binding_identity(binding: &MacroBinding) -> MacroBinding {
    let mut identity = binding.clone();
    identity.location = neutral(&binding.location);
    identity.asset.location = neutral(&binding.asset.location);
    identity.asset.locked = false;
    identity.asset.offline = false;
    for mapping in &mut identity.mappings {
        mapping.location = neutral(&mapping.location);
    }
    identity
}

fn without_origins(constraints: &[Constraint], keep_fixed: bool) -> Vec<Constraint> {
    constraints
        .iter()
        .filter(|c| keep_fixed || c.kind != ConstraintKind::FixedPlacement)
        .map(|c| Constraint { origins: Vec::new(), ..c.clone() })
        .collect()
}

/// `electrical_identity`: ignore only editable pose intent and diagnostic
/// source offsets. Every part, hierarchy, configuration, net, supply,
/// interface, dependency and other constraint must remain exactly equal.
pub fn electrical_identity(board: &Board) -> Board {
    let mut identity = board.clone();
    identity.constraints = without_origins(&board.constraints, false);
    for module in identity.module_definitions.values_mut() {
        module.constraints = without_origins(&module.constraints, true);
        module.physical_macros = module.physical_macros.iter().map(macro_binding_identity).collect();
    }
    identity.physical_macros = board.physical_macros.iter().map(macro_binding_identity).collect();
    identity
}

/// `scoped_identity`: the identity, minus the constraints a reviewed edit may change.
pub fn scoped_identity(board: &Board, scope: Option<&[Scope]>) -> Board {
    let mut identity = electrical_identity(board);
    if let Some(scopes) = scope {
        identity
            .constraints
            .retain(|c| !scopes.iter().any(|(kind, targets)| c.kind.value() == kind && c.targets == *targets));
    }
    identity
}

/// `seed_placements`: keep movable session seeds without making them locks;
/// `only` limits seeding to these references.
pub fn seed_placements(
    board: PhysicalBoard,
    seeds: &PhysicalBoard,
    only: Option<&BTreeSet<String>>,
) -> Result<PhysicalBoard> {
    let existing: IndexMap<&str, &Placement> = seeds
        .placements
        .iter()
        .filter(|p| only.is_none_or(|only| only.contains(&p.reference)))
        .map(|p| (p.reference.as_str(), p))
        .collect();
    let mut board = board;
    let rules: IndexMap<String, _> = board.placement_rules.iter().map(|r| (r.reference.clone(), r.clone())).collect();
    for pose in &mut board.placements {
        if let Some(previous) = existing.get(pose.reference.as_str())
            && previous.footprint == pose.footprint
        {
            pose.position = previous.position;
            pose.rotation_degrees = previous.rotation_degrees.clone();
            pose.side = previous.side;
        }
        if let Some(rule) = rules.get(&pose.reference) {
            pose.position = rule.fixed_position.unwrap_or(pose.position);
            if let Some(rotation) = &rule.fixed_rotation_degrees {
                pose.rotation_degrees = rotation.clone();
            }
            pose.side = rule.side.unwrap_or(pose.side);
        }
    }
    apply_fixed_cluster_poses(board)
}

/// A save holds the lock for seconds at most; an older lock was left by a crashed save.
const STALE_LOCK_SECONDS: f64 = 120.0;

fn lock_age(path: &Path) -> Option<f64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(match SystemTime::now().duration_since(modified) {
        Ok(age) => age.as_secs_f64(),
        Err(ahead) => -ahead.duration().as_secs_f64(),
    })
}

/// `_break_stale_lock`: remove a crashed save's lock, never a live one.
fn break_stale_lock(lock: &Path) {
    if lock_age(lock).is_none_or(|age| age <= STALE_LOCK_SECONDS) {
        return;
    }
    let name = lock.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let claimed = lock.with_file_name(format!("{name}.stale-{}", random_token(16, 1)));
    if std::fs::rename(lock, &claimed).is_err() {
        return;
    }
    if lock_age(&claimed).unwrap_or(0.0) <= STALE_LOCK_SECONDS {
        match std::fs::hard_link(&claimed, lock) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                if !lock.exists() && std::fs::rename(&claimed, lock).is_ok() {
                    return;
                }
            }
        }
    }
    let _ = std::fs::remove_file(&claimed);
}

/// `ReviewedSource`: a validated candidate and its diff.
#[derive(Clone)]
pub struct ReviewedSource {
    pub before: SourceSnapshot,
    pub after: SourceSnapshot,
    pub design: Design,
    pub board: Arc<PhysicalBoard>,
    pub diff: String,
    pub scope: Option<Vec<Scope>>,
    pub summary: String,
}

impl ReviewedSource {
    pub fn id(&self) -> String {
        sha256_hex(format!("{}{}", self.before.revision(), self.after.revision()).as_bytes())
    }
}

fn os(error: &std::io::Error, path: &Path) -> Error {
    Error::Os(copper_lang::paths::python_os_error(error, Some(&copper_lang::paths::display(path))))
}

impl Workspace {
    /// `validate`: compile, check identity and ERC, rebuild with the seeds, check legality.
    pub fn validate(
        &self,
        raw: &[u8],
        seeds: &PhysicalBoard,
        scope: Option<&[Scope]>,
    ) -> Result<(Design, PhysicalBoard)> {
        self.check_inputs()?;
        let snapshot = SourceSnapshot::new(raw.to_vec(), &copper_lang::paths::display(&self.source))?;
        let design = compile_design_source(&snapshot.text, &snapshot.filename, true, true)?;
        if scoped_identity(&design.electrical, scope) != scoped_identity(&self.identity, scope) {
            return value("edit changes electrical intent or dependency bytes");
        }
        let diagnostics = copper_lang::erc::check(&design.electrical, false).map_err(Error::Value)?;
        if copper_lang::erc::has_errors(&diagnostics) {
            let texts: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
            return value(format!("prospective source fails ERC: {}", texts.join("; ")));
        }
        let board = seed_placements(build(&design, &self.source, &self.settings)?, seeds, None)?;
        if !board.tracks.is_empty() || !board.vias.is_empty() || !board.materialized_macros.is_empty() {
            return value("prospective rebuild retained routed copper");
        }
        let poses = placement_map(&board);
        if !legality::placement_solution_is_legal(&board, &poses)? {
            let reasons = legality::placement_rejection_reasons(&board, &poses)?;
            return value(format!("prospective geometry/pose violates physical constraints: {}", reasons.join("; ")));
        }
        let materialized = copper_drc::macros::materialize_hard_macros(board.clone())?;
        let findings = copper_drc::placement_copper_findings(&materialized)?;
        if !findings.is_empty() {
            let texts: Vec<&str> = findings.iter().map(|f| f.message.as_str()).collect();
            return value(format!("prospective copper/drill geometry: {}", texts.join("; ")));
        }
        self.check_inputs()?;
        verify_reference_assets(&board.mechanical_references).map_err(Error::Value)?;
        Ok((design, board))
    }

    /// `review`: validate `raw` against `snapshot` and diff it.
    pub fn review(
        &self,
        snapshot: &SourceSnapshot,
        raw: Vec<u8>,
        seeds: &PhysicalBoard,
        scope: Option<Vec<Scope>>,
    ) -> Result<ReviewedSource> {
        if snapshot.raw == raw {
            return value("edit has no source changes");
        }
        let (design, board) = self.validate(&raw, seeds, scope.as_deref())?;
        let after = SourceSnapshot::new(raw, &snapshot.filename)?;
        let diff = unified_diff(
            &splitlines_keepends(&snapshot.text),
            &splitlines_keepends(&after.text),
            "current.copper",
            "proposed.copper",
        );
        Ok(ReviewedSource {
            before: snapshot.clone(),
            after,
            design,
            board: Arc::new(board),
            diff,
            scope,
            summary: String::new(),
        })
    }

    /// `save`: commit the reviewed bytes under a cooperative lock with a final byte check.
    pub fn save(&self, review: &ReviewedSource) -> Result<()> {
        let check = || {
            self.check_inputs()?;
            verify_reference_assets(&review.board.mechanical_references).map_err(Error::Value)
        };
        save_reviewed(&self.source, &review.before.raw, &review.after.raw, &check)
    }
}

/// `SourceWorkspace.save`: write `after` over `source` under a cooperative
/// lock, only while it still holds `before`; `check` re-verifies the inputs.
pub fn save_reviewed(source: &Path, before: &[u8], after: &[u8], check: &dyn Fn() -> Result<()>) -> Result<()> {
    check()?;
    if source.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        return value("source was replaced with a symbolic link");
    }
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lock = source.with_file_name(format!("{name}.editor-lock"));
    break_stale_lock(&lock);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let token = format!("pid {} token {} at {:.0}\n", std::process::id(), random_token(16, 2), now);
    let mut marker = match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return value(format!(
                "another editor is saving this source right now ({name}.editor-lock); retry in a moment. A lock older than {} s is removed automatically",
                STALE_LOCK_SECONDS as i64
            ));
        }
        Err(error) => return Err(os(&error, &lock)),
    };
    let mut temporary: Option<PathBuf> = None;
    let result = (|| -> Result<()> {
        marker.write_all(token.as_bytes()).and_then(|()| marker.flush()).map_err(|e| os(&e, &lock))?;
        let read = |path: &Path| std::fs::read(path).map_err(|e| os(&e, path));
        if read(source)? != before {
            return value("source changed since review; reload before saving");
        }
        let permissions = std::fs::metadata(source).map_err(|e| os(&e, source))?.permissions();
        let parent = source.parent().unwrap_or(Path::new("."));
        let (path, mut stream) = (0..100)
            .find_map(|attempt| {
                let path = parent.join(format!(".copper-edit-{}", random_token(4, 3 + attempt)));
                std::fs::OpenOptions::new().write(true).create_new(true).open(&path).ok().map(|f| (path, f))
            })
            .ok_or_else(|| Error::Os("[Errno 17] No usable temporary file name found".to_owned()))?;
        temporary = Some(path.clone());
        stream.write_all(after).and_then(|()| stream.sync_all()).map_err(|e| os(&e, &path))?;
        drop(stream);
        std::fs::set_permissions(&path, permissions).map_err(|e| os(&e, &path))?;
        check()?;
        if read(source)? != before {
            return value("source changed during save; no edit committed");
        }
        std::fs::rename(&path, source).map_err(|e| os(&e, source))?;
        temporary = None;
        Ok(())
    })();
    drop(marker);
    if let Some(path) = temporary {
        let _ = std::fs::remove_file(path);
    }
    if std::fs::read_to_string(&lock).is_ok_and(|text| text == token) {
        let _ = std::fs::remove_file(&lock);
    }
    result
}

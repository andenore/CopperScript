//! `pcbir.editor.project`: locked/offline editor physicalization shared by
//! the document hosts.

use std::path::{Path, PathBuf};

use copper_json::Value;
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::footprints::FootprintResolver;
use copper_lang::packages::find_manifest;
use copper_lang::paths;
use copper_lang::syntax::DocumentKind;
use copper_pcb::board::PhysicalBoard;
use copper_pcb::hard_macros::{apply_hard_macro_scene, cluster_placements};
use copper_pcb::keepouts::placement_map;
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_pcb::templates::apply_placement_templates;

use crate::configuration::physical_options;
use crate::dependencies::read_source;
use crate::overlay::RoutedOverlay;
use crate::session::{EditorSession, Workspace};
use crate::source::SourceSnapshot;
use crate::{Error, Result, value};

/// The physical inputs every rebuild of a document uses.
#[derive(Clone, Debug, Default)]
pub struct BuildSettings {
    pub footprint_roots: Vec<PathBuf>,
    pub layers: Option<i64>,
    pub fab_profile: String,
    pub templates: Option<PathBuf>,
    pub macros: Vec<PathBuf>,
    pub allow_proxy: bool,
}

/// `apply_fixed_cluster_poses`: rigid members rebuilt from a complete, source-fixed anchor.
pub fn apply_fixed_cluster_poses(mut board: PhysicalBoard) -> Result<PhysicalBoard> {
    if !board.tracks.is_empty() || !board.vias.is_empty() || !board.materialized_macros.is_empty() {
        return value("fixed cluster pose reconstruction requires an unrouted board");
    }
    let mut rules = indexmap::IndexMap::new();
    for rule in &board.placement_rules {
        rules.insert(rule.reference.clone(), rule.clone());
    }
    let mut poses = placement_map(&board);
    for cluster in &board.rigid_clusters {
        let Some(rule) = rules.get(&cluster.anchor_reference) else { continue };
        let (Some(position), Some(rotation), Some(side)) =
            (rule.fixed_position, &rule.fixed_rotation_degrees, rule.side)
        else {
            continue;
        };
        let mut anchor = poses[&cluster.anchor_reference].clone();
        anchor.position = position;
        anchor.rotation_degrees = rotation.clone();
        anchor.side = side;
        let transformed = cluster_placements(cluster, &anchor, &poses)?;
        for (reference, pose) in &transformed {
            if let Some(member) = rules.get(reference)
                && (member.fixed_position.is_some_and(|p| p != pose.position)
                    || member.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != pose.rotation_degrees)
                    || member.side.is_some_and(|s| s != pose.side))
            {
                return value(format!("{}: fixed anchor conflicts with fixed member {reference}", cluster.name));
            }
        }
        poses.extend(transformed);
    }
    board.placements = board.placements.iter().map(|p| poses[&p.reference].clone()).collect();
    Ok(board)
}

/// `build`: physicalize a candidate design with the session's settings.
pub fn build(design: &Design, source: &Path, settings: &BuildSettings) -> Result<PhysicalBoard> {
    let options = physical_options(design, settings.layers, &settings.fab_profile)?;
    let mut board = if settings.allow_proxy {
        physicalize(design, Footprints::Proxy, &options, true, true)?
    } else {
        let base = source.parent().map(Path::to_path_buf).unwrap_or_default();
        let roots: Vec<PathBuf> = settings.footprint_roots.iter().map(|p| paths::resolve(p)).collect();
        let mut resolver = FootprintResolver::new(&base, &roots, false, true, true);
        physicalize(design, Footprints::Resolved(&mut resolver), &options, true, true)?
    };
    if let Some(templates) = &settings.templates {
        board = apply_placement_templates(board, templates, true, true)?;
    }
    for scene in &settings.macros {
        board = apply_hard_macro_scene(board, scene, true, true)?;
    }
    apply_fixed_cluster_poses(board)
}

/// `source_macro_input_paths`: every recorded macro input, first occurrence first.
fn source_macro_input_paths(board: &PhysicalBoard) -> Result<Vec<PathBuf>> {
    let text = board.metadata.get("source_physical_macros").map_or("[]", String::as_str);
    let rows = copper_json::parse(text).map_err(|e| Error::Uncaught(format!("JSONDecodeError: {e}")))?;
    let mut result: Vec<PathBuf> = Vec::new();
    for row in rows.as_array().unwrap_or(&[]) {
        let paths: Vec<String> = match row.get("inputs") {
            Some(Value::Object(inputs)) => inputs.keys().cloned().collect(),
            _ => row.get("source_path").and_then(Value::as_str).map(str::to_owned).into_iter().collect(),
        };
        for path in paths {
            let path = PathBuf::from(path);
            if !result.contains(&path) {
                result.push(path);
            }
        }
    }
    Ok(result)
}

/// `document_session`: the editor session of a board read from disk.
pub fn document_session(
    source: &Path,
    settings: &BuildSettings,
    overlay: Option<RoutedOverlay>,
) -> Result<EditorSession> {
    let source = paths::resolve(source);
    let filename = paths::display(&source);
    let raw = read_source(&source)?;
    let snapshot = SourceSnapshot::new(raw, &filename)?;
    let document = copper_lang::parse(&snapshot.text, &filename)?;
    if matches!(document.kind, DocumentKind::Macro | DocumentKind::BoardProfile) {
        if overlay.is_some() || settings.templates.is_some() || !settings.macros.is_empty() {
            return value("board overlays and placement assets require a board document");
        }
        return Err(Error::NotImplemented("edit-mechanical definition sessions (macro and board_profile)".to_owned()));
    }
    if document.kind != DocumentKind::Board {
        return value("physical editor supports board, macro and board_profile documents");
    }
    let design = compile_design_source(&snapshot.text, &filename, true, true)?;
    let diagnostics = copper_lang::erc::check(&design.electrical, false).map_err(Error::Value)?;
    if copper_lang::erc::has_errors(&diagnostics) {
        let texts: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        return value(format!("source fails ERC: {}", texts.join("; ")));
    }
    let board = build(&design, &source, settings)?;
    let mut inputs: Vec<PathBuf> = settings.templates.iter().chain(&settings.macros).cloned().collect();
    if let Some(manifest) = find_manifest(source.parent().unwrap_or(Path::new("/"))) {
        let lock = manifest.parent().unwrap_or(Path::new("/")).join("copper.lock");
        inputs.push(manifest);
        inputs.push(lock);
    }
    inputs.extend(
        board
            .footprints
            .values()
            .filter_map(|f| f.metadata.get("source_path"))
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    );
    inputs.extend(source_macro_input_paths(&board)?);
    let workspace = Workspace::new(&source, &design, inputs)?;
    EditorSession::new(board, &source, Some(workspace), overlay)
}

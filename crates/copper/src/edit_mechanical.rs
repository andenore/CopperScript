//! `copper edit-mechanical BOARD` (alias `edit`): the physical editor.
//!
//! `--scene-output` writes the opened session's derived scene and exits; the
//! loopback HTTP editor is not implemented yet and reports so.

use std::io::Write;
use std::path::{Path, PathBuf};

use copper_editor::Error;
use copper_editor::overlay::RoutedOverlay;
use copper_editor::project::{BuildSettings, document_session};
use copper_json::{Dumps, Value, text};
use copper_lang::paths;

use crate::commands::{NOT_IMPLEMENTED_EXIT, NOT_IMPLEMENTED_MARKER};
use crate::pypath;
use crate::report_args::{self, Opt, python_int};

const USAGE: &str = "usage: copper edit-mechanical [-h] [--project PROJECT] [--locked] [--offline]
                              [--footprint-root FOOTPRINT_ROOT]
                              [--layers {2,4,6}]
                              [--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}]
                              [--placement-templates PLACEMENT_TEMPLATES]
                              [--hard-macro HARD_MACRO]
                              [--allow-proxy-footprints] [--no-browser]
                              [--port PORT] [--scene-output SCENE_OUTPUT]
                              [--overlay OVERLAY]
                              board";

/// argparse's `--help` text.
const HELP: &str = "usage: copper edit-mechanical [-h] [--project PROJECT] [--locked] [--offline]
                              [--footprint-root FOOTPRINT_ROOT]
                              [--layers {2,4,6}]
                              [--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}]
                              [--placement-templates PLACEMENT_TEMPLATES]
                              [--hard-macro HARD_MACRO]
                              [--allow-proxy-footprints] [--no-browser]
                              [--port PORT] [--scene-output SCENE_OUTPUT]
                              [--overlay OVERLAY]
                              board

positional arguments:
  board

options:
  -h, --help            show this help message and exit
  --project PROJECT     consumer project whose copper.mod pins a library
                        definition's dependencies
  --locked              require copper.lock to match every package byte
  --offline             reject remote package cache misses
  --footprint-root FOOTPRINT_ROOT
  --layers {2,4,6}      explicit copper-layer count; default derives it from
                        source stackup and layer requirements
  --fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}
  --placement-templates PLACEMENT_TEMPLATES
  --hard-macro HARD_MACRO
  --allow-proxy-footprints
                        explicit inspection-only proxy geometry
  --no-browser
  --port PORT           loopback port; zero chooses an available port
  --scene-output SCENE_OUTPUT
                        write a derived scene and exit without serving
  --overlay OVERLAY     optional content-bound routed reference JSON";

const PROFILES: [&str; 3] = ["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"];

fn not_implemented(err: &mut dyn Write, what: &str) -> u8 {
    let _ = writeln!(err, "copper: {NOT_IMPLEMENTED_MARKER}: edit-mechanical ({what})");
    NOT_IMPLEMENTED_EXIT
}

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper edit-mechanical: error: {message}");
        2
    };
    let options = [
        report_args::valued("--project"),
        report_args::flag("--locked"),
        report_args::flag("--offline"),
        report_args::valued("--footprint-root"),
        report_args::valued("--layers"),
        report_args::valued("--fab-profile"),
        report_args::valued("--placement-templates"),
        report_args::valued("--hard-macro"),
        report_args::flag("--allow-proxy-footprints"),
        report_args::flag("--no-browser"),
        report_args::valued("--port"),
        report_args::valued("--scene-output"),
        Opt { long: "--overlay", short: None, valued: true },
    ];
    let parsed = match report_args::parse(arguments, &options) {
        Ok(Some(parsed)) => parsed,
        Ok(None) => {
            let _ = text::print(out, HELP);
            return 0;
        }
        Err(message) => return usage_error(err, &message),
    };
    let layers = match parsed.last("--layers") {
        None => None,
        Some(value) => match python_int(value) {
            Some(count) if [2, 4, 6].contains(&count) => Some(count),
            Some(count) => {
                return usage_error(
                    err,
                    &format!("argument --layers: invalid choice: '{count}' (choose from '2', '4', '6')"),
                );
            }
            None => return usage_error(err, &format!("argument --layers: invalid int value: '{value}'")),
        },
    };
    let profile = parsed.last("--fab-profile").unwrap_or("generic").to_owned();
    if !PROFILES.contains(&profile.as_str()) {
        return usage_error(
            err,
            &format!("argument --fab-profile: invalid choice: '{profile}' (choose from '{}')", PROFILES.join("', '")),
        );
    }
    if let Some(port) = parsed.last("--port")
        && python_int(port).is_none()
    {
        return usage_error(err, &format!("argument --port: invalid int value: '{port}'"));
    }
    let board = match parsed.positionals.as_slice() {
        [board] => PathBuf::from(pypath::display(board)),
        [] => return usage_error(err, "the following arguments are required: board"),
        [_, extra, ..] => return usage_error(err, &format!("unrecognized arguments: {extra}")),
    };
    let scene_output = parsed.last("--scene-output").map(|p| PathBuf::from(pypath::display(p)));
    let port = parsed.last("--port").and_then(python_int).unwrap_or(0);
    let path_list =
        |name: &str| -> Vec<PathBuf> { parsed.all(name).iter().map(|p| PathBuf::from(pypath::display(p))).collect() };
    let settings = BuildSettings {
        footprint_roots: path_list("--footprint-root"),
        layers,
        fab_profile: profile,
        templates: parsed.last("--placement-templates").map(|p| PathBuf::from(pypath::display(p))),
        macros: path_list("--hard-macro"),
        allow_proxy: parsed.has("--allow-proxy-footprints"),
    };
    let project = parsed.last("--project").map(|p| PathBuf::from(pypath::display(p)));
    let overlay = parsed.last("--overlay").map(|p| PathBuf::from(pypath::display(p)));
    let serving = scene_output.is_none();
    let progress = |out: &mut dyn Write, message: &str| {
        if serving {
            let _ = text::print(out, message);
            let _ = out.flush();
        }
    };
    let mut server = None;
    let outcome = (|| -> copper_editor::Result<()> {
        progress(out, "Opening board: checking source and dependencies...");
        copper_editor::dependencies::prepare_editor_dependencies(
            &board,
            parsed.has("--offline"),
            parsed.has("--locked"),
            project.as_deref(),
            &mut |message| progress(out, message),
        )?;
        let overlay = match &overlay {
            Some(path) => Some(RoutedOverlay::load(path)?),
            None => None,
        };
        progress(out, "Opening board: resolving footprints and building the physical board...");
        let mut session = document_session(&board, &settings, overlay, project.as_deref())?;
        progress(out, "Opening board: editor session ready.");
        let Some(scene_output) = &scene_output else {
            server = Some(copper_editor::server::create_server(session, &port.into())?);
            return Ok(());
        };
        let scene_output = scene_output.clone();
        if paths::resolve(&scene_output) == paths::resolve(&board) {
            return Err(Error::Value("scene output cannot overwrite the source board".to_owned()));
        }
        let mut file = create_new(&scene_output)?;
        let scene = Value::Object(session.scene()?);
        let body = format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&scene));
        file.write_all(body.as_bytes()).map_err(|error| os(error, &scene_output))
    })();
    match outcome {
        Ok(()) => match (server, &scene_output) {
            (Some(server), _) => {
                let _ = text::print(out, &format!("Physical editor (reviewed source editing): {}", server.url()));
                let _ = text::print(
                    out,
                    "Placement previews are temporary; source changes require explicit review/save. Ctrl+C stops the local service.",
                );
                let _ = out.flush();
                if !parsed.has("--no-browser") {
                    open_browser(&server.url());
                }
                copper_editor::server::serve(server, &exit_not_implemented);
                0
            }
            (None, Some(scene_output)) => {
                let _ = text::print(
                    out,
                    &format!("Derived editor scene -> {}; source unchanged", paths::display(scene_output)),
                );
                0
            }
            (None, None) => unreachable!("a server or a scene"),
        },
        Err(Error::NotImplemented(what)) => not_implemented(err, &what),
        Err(error) => {
            let _ = text::print(out, &format!("MECHANICAL EDITOR ERROR: {error}"));
            2
        }
    }
}

/// A request reached behaviour this implementation lacks: exit 3 with the marker.
fn exit_not_implemented(what: &str) {
    let _ = std::io::stdout().flush();
    eprintln!("copper: {NOT_IMPLEMENTED_MARKER}: edit-mechanical ({what})");
    std::process::exit(i32::from(NOT_IMPLEMENTED_EXIT));
}

/// `webbrowser.open`: best effort, silent.
fn open_browser(url: &str) {
    let command = if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(command.0)
        .args(command.1)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn os(error: std::io::Error, path: &Path) -> Error {
    Error::Os(paths::python_os_error(&error, Some(&paths::display(path))))
}

/// `mkdir(parents=True, exist_ok=True)` then `open("x")`: never overwritten.
fn create_new(path: &Path) -> copper_editor::Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| os(error, parent))?;
    }
    std::fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|error| os(error, path))
}

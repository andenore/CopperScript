//! `copper render PLOT_DIR -o OUTPUT.svg --layer SPEC [--layer SPEC]...
//! [--background COLOR] [--margin MM] [--title TEXT]`: an SVG illustration
//! of a native (or KiCad) Gerber plot, layer by layer, without KiCad.

use std::io::Write;
use std::path::PathBuf;

use copper_cam::render::{Options, Plate, load, svg};
use copper_json::text;
use copper_lang::paths;

use crate::option_parser::{self, short_value, value};
use crate::pypath;

const USAGE: &str = "usage: copper render [-h] -o OUTPUT --layer LAYER[&WITHIN][:COLOR[:OPACITY]] [--layer ...] \
[--background COLOR] [--margin MM] [--title TITLE] plot_dir";

const ABOUT: &str = "draw the Gerbers of a plot directory (copper plot, or KiCad's) as one SVG, viewed from above:
each --layer is a coloured plate drawn in order, bottom first, from its plotted artwork (dark objects
add, clear objects remove). LAYER&WITHIN draws LAYER only where WITHIN is dark, e.g. F.Cu&F.Mask for
exposed top copper. Layers are named as in KiCad (F.Cu, In1.Cu, B.Cu, F.Mask, F.Silkscreen, Edge.Cuts...)
from each file's TF.FileFunction. The frame is the board profile (Edge.Cuts) plus --margin (default 1 mm).";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper render: error: {message}");
        2
    };
    let specs =
        [short_value("--output", "-o"), value("--layer"), value("--background"), value("--margin"), value("--title")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(out, "{USAGE}\n\n{ABOUT}");
        crate::help::options(out, "render");
        return 0;
    }
    let (directory, output) = match (parsed.positionals.as_slice(), parsed.value("--output")) {
        ([directory], Some(output)) => {
            (PathBuf::from(pypath::display(directory)), PathBuf::from(pypath::display(output)))
        }
        ([_, extra @ ..], _) if !extra.is_empty() => {
            return usage_error(err, &format!("unrecognized arguments: {}", extra.join(" ")));
        }
        _ => return usage_error(err, "the following arguments are required: plot_dir, -o/--output"),
    };
    let mut plates = Vec::new();
    for spec in parsed.values("--layer") {
        match Plate::parse(spec) {
            Ok(plate) => plates.push(plate),
            Err(message) => return usage_error(err, &format!("--layer {spec}: {message}")),
        }
    }
    if plates.is_empty() {
        return usage_error(err, "at least one --layer is required");
    }
    let background = parsed.value("--background").map(str::to_owned);
    if let Some(color) = &background
        && Plate::parse(&format!("x:{color}")).is_err()
    {
        return usage_error(err, &format!("--background must be #rgb or #rrggbb, not {color:?}"));
    }
    let margin_mm = match parsed.value("--margin").map(str::parse::<f64>) {
        None => 1.0,
        Some(Ok(margin)) if margin >= 0.0 && margin.is_finite() => margin,
        Some(_) => return usage_error(err, "--margin must be a non-negative number of millimetres"),
    };
    let title = parsed.value("--title").map(str::to_owned).unwrap_or_else(|| {
        format!(
            "{} ({})",
            directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            plates.iter().map(|plate| plate.layer.as_str()).collect::<Vec<_>>().join(", ")
        )
    });
    let fail = |out: &mut dyn Write, message: String| {
        let _ = text::print(out, &format!("RENDER ERROR: {message}"));
        2
    };
    let images = match load(&directory) {
        Ok(images) => images,
        Err(message) => return fail(out, message),
    };
    let picture = match svg(&images, &plates, &Options { background, margin_mm, title }) {
        Ok(picture) => picture,
        Err(message) => return fail(out, message),
    };
    if let Some(parent) = output.parent().filter(|parent| !parent.as_os_str().is_empty())
        && let Err(failure) = std::fs::create_dir_all(parent)
    {
        return fail(out, format!("cannot create {}: {failure}", parent.display()));
    }
    if let Err(failure) = std::fs::write(&output, &picture) {
        return fail(out, format!("cannot write {}: {failure}", paths::display(&output)));
    }
    let _ = text::print(
        out,
        &format!("Rendered {} layer plate(s) -> {} ({} bytes)", plates.len(), paths::display(&output), picture.len()),
    );
    0
}

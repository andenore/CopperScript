//! `copper compare-cam REFERENCE CANDIDATE`: rasterise and match two sets of
//! manufacturing files (Gerbers by X2 file function, drills, IPC-D-356,
//! positions). Exit 0 when they match, 1 when they differ.

use std::io::Write;
use std::path::PathBuf;

use copper_cam::compare::{self, Options};
use copper_json::{Dumps, text};

use crate::option_parser::{self, value};
use crate::pypath;

const USAGE: &str = "usage: copper compare-cam [-h] [--pitch-um PITCH_UM] [--erosion PIXELS] \
[--hole-tolerance-um UM] [--report REPORT] reference candidate";

pub fn run(arguments: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let usage_error = |err: &mut dyn Write, message: &str| {
        let _ = writeln!(err, "{USAGE}\ncopper compare-cam: error: {message}");
        2
    };
    let specs = [value("--pitch-um"), value("--erosion"), value("--hole-tolerance-um"), value("--report")];
    let parsed = match option_parser::parse(arguments, &specs) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(err, &message),
    };
    if parsed.flag("--help") {
        let _ = writeln!(
            out,
            "{USAGE}\n\ncompare two manufacturing file sets: Gerbers rasterised per X2 file function (default 10 um pitch,\n1 pixel erosion), drill hits within a tolerance (default 2 um), IPC-D-356 records and positions exactly"
        );
        return 0;
    }
    let [reference, candidate] = parsed.positionals.as_slice() else {
        return usage_error(err, "the following arguments are required: reference, candidate");
    };
    let mut options = Options::default();
    let number = |name: &str| -> Result<Option<f64>, String> {
        parsed
            .value(name)
            .map(|text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value > 0.0)
                    .ok_or(format!("{name} must be a positive number"))
            })
            .transpose()
    };
    match (
        number("--pitch-um"),
        number("--hole-tolerance-um"),
        parsed.value("--erosion").map(str::parse::<usize>).transpose(),
    ) {
        (Ok(pitch), Ok(tolerance), Ok(erosion)) => {
            if let Some(pitch) = pitch {
                options.pitch = pitch * 1000.0;
            }
            if let Some(tolerance) = tolerance {
                options.hole_tolerance = tolerance * 1000.0;
            }
            if let Some(erosion) = erosion {
                options.erosion = erosion;
            }
        }
        (Err(message), _, _) | (_, Err(message), _) => return usage_error(err, &message),
        (_, _, Err(_)) => return usage_error(err, "--erosion must be a whole number of pixels"),
    }
    let (reference, candidate) = (PathBuf::from(pypath::display(reference)), PathBuf::from(pypath::display(candidate)));
    let report = match compare::compare(&reference, &candidate, &options) {
        Ok(report) => report,
        Err(failure) => {
            let _ = text::print(out, &format!("CAM COMPARISON ERROR: {failure}"));
            return 2;
        }
    };
    if let Some(path) = parsed.value("--report") {
        let json = Dumps::new().indent(2).dumps(&report) + "\n";
        if let Err(failure) = std::fs::write(pypath::display(path), json) {
            let _ = text::print(out, &format!("CAM COMPARISON ERROR: cannot write {path}: {failure}"));
            return 2;
        }
    }
    let _ = text::print(out, &compare::summary(&report));
    if matches!(report.get("matches"), Some(copper_json::Value::Bool(true))) { 0 } else { 1 }
}

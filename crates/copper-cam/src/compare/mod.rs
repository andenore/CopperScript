//! CAM comparison of two manufacturing file sets, typically KiCad's plot and
//! the native plot of the same board.
//!
//! Gerbers pair up by `TF.FileFunction` and are rasterised on one shared
//! grid; a layer matches when no difference survives erosion by the
//! boundary tolerance (edges rasterised from differently approximated arcs
//! or rounded coordinates differ by about a pixel). Drill hits match within
//! a distance tolerance, IPC-D-356 records and positions must agree exactly
//! (positions to 0.001 mm / degree).

pub mod drill;
pub mod gerber;
pub mod netlist;
pub mod raster;

use std::path::{Path, PathBuf};

use copper_json::{Object, Value};
use indexmap::IndexMap;

use drill::Hit;
use gerber::{Image, Op};
use netlist::{Position, Record};
use raster::{Frame, Raster};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Raster pitch in nanometres.
    pub pitch: f64,
    /// Erosion radius in pixels applied to the difference.
    pub erosion: usize,
    /// Drill position and diameter tolerance in nanometres.
    pub hole_tolerance: f64,
    /// Most differences listed per item.
    pub limit: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { pitch: 10_000.0, erosion: 1, hole_tolerance: 2_000.0, limit: 10 }
    }
}

#[derive(Default)]
struct FileSet {
    gerbers: IndexMap<String, (String, Image)>,
    hits: Vec<Hit>,
    drill_files: Vec<String>,
    netlist: Option<(String, Vec<Record>)>,
    positions: Option<(String, IndexMap<String, Position>)>,
}

fn walk(root: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(root).map_err(|failure| format!("cannot list {}: {failure}", root.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            walk(&path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

fn load(root: &Path) -> Result<FileSet, String> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    let mut set = FileSet::default();
    for path in files {
        let name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let read = || std::fs::read_to_string(&path).map_err(|failure| format!("cannot read {name}: {failure}"));
        match extension.as_str() {
            "gbr" | "gtl" | "gbl" | "gts" | "gbs" | "gto" | "gbo" | "gtp" | "gbp" | "gm1" => {
                let image = gerber::read(&read()?).map_err(|failure| format!("{name}: {failure}"))?;
                let key = image.file_function.clone().unwrap_or_else(|| name.clone());
                if let Some((other, _)) = set.gerbers.get(&key) {
                    return Err(format!("{name} and {other} both hold {key}"));
                }
                set.gerbers.insert(key, (name, image));
            }
            "drl" | "xln" | "exc" => {
                set.hits.extend(drill::read(&read()?, &name).map_err(|failure| format!("{name}: {failure}"))?);
                set.drill_files.push(name);
            }
            "d356" | "ipc" => {
                if set.netlist.is_some() {
                    return Err(format!("{}: more than one IPC-D-356 netlist", root.display()));
                }
                set.netlist = Some((
                    name.clone(),
                    netlist::read_ipc356(&read()?).map_err(|failure| format!("{name}: {failure}"))?,
                ));
            }
            "csv" => {
                let text = read()?;
                if text.starts_with("Ref,Val,Package,PosX,PosY,Rot,Side") {
                    if set.positions.is_some() {
                        return Err(format!("{}: more than one position file", root.display()));
                    }
                    set.positions = Some((
                        name.clone(),
                        netlist::read_positions(&text).map_err(|failure| format!("{name}: {failure}"))?,
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(set)
}

fn rasterise(image: &Image, frame: Frame) -> Raster {
    let mut raster = Raster::new(frame);
    for op in &image.ops {
        match op {
            Op::Disk { center, radius, dark } => raster.disk(center.0, center.1, *radius, *dark),
            Op::Polygon { points, dark } => raster.polygon(points, *dark),
            Op::Capsule { a, b, width, dark } => raster.capsule(*a, *b, *width, *dark),
        }
    }
    raster
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

fn mm(nm: f64) -> Value {
    Value::from(round4(nm / 1e6))
}

fn compare_layer(reference: &Image, candidate: &Image, options: &Options) -> Value {
    let bounds = match (reference.bounds(), candidate.bounds()) {
        (Some(a), Some(b)) => (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => (0.0, 0.0, 1.0, 1.0),
    };
    let frame = Frame::covering(bounds, options.pitch);
    let (a, b) = (rasterise(reference, frame), rasterise(candidate, frame));
    let difference = a.xor(&b);
    let significant = difference.eroded(options.erosion);
    let pixel_area = options.pitch * options.pitch / 1e12;
    let (count, blobs) = significant.components(options.limit);
    let blobs: Vec<Value> = blobs
        .into_iter()
        .map(|(pixels, x0, y0, x1, y1)| {
            // Grown back by the erosion radius; board coordinates, Y down.
            let grow = options.erosion as f64 * frame.pitch;
            let gx = |col: usize| frame.x0 + col as f64 * frame.pitch;
            let gy = |row: usize| frame.y0 + row as f64 * frame.pitch;
            let reference_dark = a.get((x0 + x1) / 2, (y0 + y1) / 2);
            object(vec![
                ("area_mm2", Value::from(round4(pixels as f64 * pixel_area))),
                ("x_min_mm", mm(gx(x0) - grow)),
                ("y_min_mm", mm(-(gy(y1 + 1) + grow))),
                ("x_max_mm", mm(gx(x1 + 1) + grow)),
                ("y_max_mm", mm(-(gy(y0) - grow))),
                ("only_in", Value::from(if reference_dark { "reference" } else { "candidate" })),
            ])
        })
        .collect();
    object(vec![
        ("matches", Value::Bool(significant.count() == 0)),
        ("reference_area_mm2", Value::from(round4(a.count() as f64 * pixel_area))),
        ("candidate_area_mm2", Value::from(round4(b.count() as f64 * pixel_area))),
        ("difference_area_mm2", Value::from(round4(difference.count() as f64 * pixel_area))),
        ("significant_area_mm2", Value::from(round4(significant.count() as f64 * pixel_area))),
        ("significant_regions", Value::from(count as u64)),
        ("largest_regions", Value::Array(blobs)),
    ])
}

fn hit_text(hit: &Hit) -> String {
    let point = |p: (f64, f64)| format!("({:.4}, {:.4})", p.0 / 1e6, -p.1 / 1e6);
    let kind = if hit.plated { "plated" } else { "non-plated" };
    match hit.end {
        None => format!("{kind} {:.4} mm at {}", hit.diameter / 1e6, point(hit.start)),
        Some(end) => format!("{kind} {:.4} mm slot {} - {}", hit.diameter / 1e6, point(hit.start), point(end)),
    }
}

fn hits_match(a: &Hit, b: &Hit, tolerance: f64) -> bool {
    let near = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).abs() <= tolerance && (p.1 - q.1).abs() <= tolerance;
    a.plated == b.plated
        && (a.diameter - b.diameter).abs() <= tolerance
        && match (a.end, b.end) {
            (None, None) => near(a.start, b.start),
            (Some(ae), Some(be)) => {
                (near(a.start, b.start) && near(ae, be)) || (near(a.start, be) && near(ae, b.start))
            }
            _ => false,
        }
}

fn compare_hits(reference: &[Hit], candidate: &[Hit], options: &Options) -> Value {
    let grid = options.hole_tolerance.max(1.0) * 4.0;
    let key = |hit: &Hit| {
        let centre = match hit.end {
            None => hit.start,
            Some(end) => ((hit.start.0 + end.0) / 2.0, (hit.start.1 + end.1) / 2.0),
        };
        ((centre.0 / grid).round() as i64, (centre.1 / grid).round() as i64)
    };
    let mut buckets: IndexMap<(i64, i64), Vec<usize>> = IndexMap::new();
    for (index, hit) in candidate.iter().enumerate() {
        buckets.entry(key(hit)).or_default().push(index);
    }
    let mut used = vec![false; candidate.len()];
    let mut missing = Vec::new();
    for hit in reference {
        let (kx, ky) = key(hit);
        let found = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dy| (kx + dx, ky + dy)))
            .filter_map(|cell| buckets.get(&cell))
            .flatten()
            .copied()
            .find(|&index| !used[index] && hits_match(hit, &candidate[index], options.hole_tolerance));
        match found {
            Some(index) => used[index] = true,
            None => missing.push(hit_text(hit)),
        }
    }
    let extra: Vec<String> =
        candidate.iter().zip(&used).filter(|(_, used)| !**used).map(|(hit, _)| hit_text(hit)).collect();
    object(vec![
        ("matches", Value::Bool(missing.is_empty() && extra.is_empty())),
        ("reference_hits", Value::from(reference.len() as u64)),
        ("candidate_hits", Value::from(candidate.len() as u64)),
        ("only_in_reference", Value::from(missing.len() as u64)),
        ("only_in_candidate", Value::from(extra.len() as u64)),
        (
            "examples_only_in_reference",
            Value::Array(missing.into_iter().take(options.limit).map(Value::from).collect()),
        ),
        ("examples_only_in_candidate", Value::Array(extra.into_iter().take(options.limit).map(Value::from).collect())),
    ])
}

/// Netlists match when the same test points exist on both sides and group
/// into the same nets. Net names are labels: a net named differently on the
/// two sides (KiCad truncates names longer than the 14-column field) is listed
/// but does not fail the comparison.
fn compare_netlists(reference: &[Record], candidate: &[Record], options: &Options) -> Value {
    type Point = (String, String, String, bool, String, Vec<String>);
    let point = |record: &Record| -> Point {
        (
            record.code.clone(),
            record.reference.clone(),
            record.pin.clone(),
            record.midpoint,
            record.hole.clone(),
            record.fields.clone(),
        )
    };
    let mut counts: IndexMap<Point, i64> = IndexMap::new();
    for record in reference {
        *counts.entry(point(record)).or_default() += 1;
    }
    for record in candidate {
        *counts.entry(point(record)).or_default() -= 1;
    }
    let text = |key: &Point| {
        format!("{} {}-{}{} {} {}", key.0, key.1, key.2, if key.3 { " M" } else { "" }, key.4, key.5.join(""))
    };
    let missing: Vec<String> = counts.iter().filter(|(_, count)| **count > 0).map(|(key, _)| text(key)).collect();
    let extra: Vec<String> = counts.iter().filter(|(_, count)| **count < 0).map(|(key, _)| text(key)).collect();
    // Net partition: each reference net must map to exactly one candidate net
    // and back, over the test points present on both sides.
    let nets = |records: &[Record]| -> IndexMap<Point, String> {
        records.iter().map(|record| (point(record), record.net.clone())).collect()
    };
    let (a, b) = (nets(reference), nets(candidate));
    let mut forward: IndexMap<&String, IndexMap<&String, usize>> = IndexMap::new();
    let mut backward: IndexMap<&String, IndexMap<&String, usize>> = IndexMap::new();
    for (key, net) in &a {
        if let Some(other) = b.get(key) {
            *forward.entry(net).or_default().entry(other).or_default() += 1;
            *backward.entry(other).or_default().entry(net).or_default() += 1;
        }
    }
    let mut split = Vec::new();
    for (net, targets) in &forward {
        if targets.len() > 1 {
            let names: Vec<String> = targets.keys().map(|name| name.to_string()).collect();
            split.push(format!("reference net {net} is split into {}", names.join(", ")));
        }
    }
    for (net, sources) in &backward {
        if sources.len() > 1 {
            let names: Vec<String> = sources.keys().map(|name| name.to_string()).collect();
            split.push(format!("candidate net {net} merges reference nets {}", names.join(", ")));
        }
    }
    let renamed: Vec<String> = forward
        .iter()
        .filter(|(net, targets)| targets.len() == 1 && targets.keys().next() != Some(*net))
        .map(|(net, targets)| format!("{net} -> {}", targets.keys().next().expect("one target")))
        .collect();
    object(vec![
        ("matches", Value::Bool(missing.is_empty() && extra.is_empty() && split.is_empty())),
        ("reference_records", Value::from(reference.len() as u64)),
        ("candidate_records", Value::from(candidate.len() as u64)),
        ("only_in_reference", Value::from(missing.len() as u64)),
        ("only_in_candidate", Value::from(extra.len() as u64)),
        ("connectivity_differences", Value::from(split.len() as u64)),
        ("renamed_nets", Value::Array(renamed.into_iter().map(Value::from).collect())),
        (
            "examples_only_in_reference",
            Value::Array(missing.into_iter().take(options.limit).map(Value::from).collect()),
        ),
        ("examples_only_in_candidate", Value::Array(extra.into_iter().take(options.limit).map(Value::from).collect())),
        ("examples_connectivity", Value::Array(split.into_iter().take(options.limit).map(Value::from).collect())),
    ])
}

fn compare_positions(
    reference: &IndexMap<String, Position>,
    candidate: &IndexMap<String, Position>,
    options: &Options,
) -> Value {
    let mut differences = Vec::new();
    for (name, a) in reference {
        match candidate.get(name) {
            None => differences.push(format!("{name}: only in reference")),
            Some(b) => {
                let rotation = ((a.rotation - b.rotation) % 360.0 + 360.0) % 360.0;
                let rotation = rotation.min(360.0 - rotation);
                if a.value != b.value
                    || a.package != b.package
                    || a.side != b.side
                    || (a.x - b.x).abs() > 0.001
                    || (a.y - b.y).abs() > 0.001
                    || rotation > 0.001
                {
                    differences.push(format!(
                        "{name}: {} {} ({}, {}) {} {} vs {} {} ({}, {}) {} {}",
                        a.value,
                        a.package,
                        a.x,
                        a.y,
                        a.rotation,
                        a.side,
                        b.value,
                        b.package,
                        b.x,
                        b.y,
                        b.rotation,
                        b.side
                    ));
                }
            }
        }
    }
    differences.extend(
        candidate.keys().filter(|name| !reference.contains_key(*name)).map(|name| format!("{name}: only in candidate")),
    );
    object(vec![
        ("matches", Value::Bool(differences.is_empty())),
        ("reference_rows", Value::from(reference.len() as u64)),
        ("candidate_rows", Value::from(candidate.len() as u64)),
        ("differences", Value::from(differences.len() as u64)),
        ("examples", Value::Array(differences.into_iter().take(options.limit).map(Value::from).collect())),
    ])
}

/// Compare the manufacturing files under `reference` with those under
/// `candidate`. The report's `matches` is true when every part matches.
pub fn compare(reference: &Path, candidate: &Path, options: &Options) -> Result<Value, String> {
    let (a, b) = (load(reference)?, load(candidate)?);
    let mut layers = Object::new();
    let mut all = true;
    let keys: Vec<&String> =
        a.gerbers.keys().chain(b.gerbers.keys().filter(|key| !a.gerbers.contains_key(*key))).collect();
    for key in keys {
        let value = match (a.gerbers.get(key), b.gerbers.get(key)) {
            (Some((ra, ia)), Some((rb, ib))) => {
                let mut value = compare_layer(ia, ib, options);
                if let Value::Object(fields) = &mut value {
                    fields.insert("reference_file".to_owned(), Value::from(ra.as_str()));
                    fields.insert("candidate_file".to_owned(), Value::from(rb.as_str()));
                }
                value
            }
            (Some((name, _)), None) => {
                object(vec![("matches", Value::Bool(false)), ("only_in_reference", Value::from(name.as_str()))])
            }
            (None, Some((name, _))) => {
                object(vec![("matches", Value::Bool(false)), ("only_in_candidate", Value::from(name.as_str()))])
            }
            (None, None) => unreachable!(),
        };
        all &= matches!(value.get("matches"), Some(Value::Bool(true)));
        layers.insert(key.clone(), value);
    }
    let drills = compare_hits(&a.hits, &b.hits, options);
    let netlist = match (&a.netlist, &b.netlist) {
        (Some((_, ra)), Some((_, rb))) => compare_netlists(ra, rb, options),
        (None, None) => {
            object(vec![("matches", Value::Bool(true)), ("note", Value::from("no netlist on either side"))])
        }
        _ => object(vec![("matches", Value::Bool(false)), ("note", Value::from("netlist on one side only"))]),
    };
    let positions = match (&a.positions, &b.positions) {
        (Some((_, pa)), Some((_, pb))) => compare_positions(pa, pb, options),
        (None, None) => {
            object(vec![("matches", Value::Bool(true)), ("note", Value::from("no positions on either side"))])
        }
        _ => object(vec![("matches", Value::Bool(false)), ("note", Value::from("positions on one side only"))]),
    };
    for part in [&drills, &netlist, &positions] {
        all &= matches!(part.get("matches"), Some(Value::Bool(true)));
    }
    Ok(object(vec![
        ("schema", Value::from("copperscript-cam-comparison/v0.1")),
        ("matches", Value::Bool(all)),
        ("raster_pitch_um", Value::from(round4(options.pitch / 1000.0))),
        ("erosion_pixels", Value::from(options.erosion as u64)),
        ("hole_tolerance_um", Value::from(round4(options.hole_tolerance / 1000.0))),
        ("layers", Value::Object(layers)),
        ("drills", drills),
        ("netlist", netlist),
        ("positions", positions),
    ]))
}

/// A short human summary of a comparison report.
pub fn summary(report: &Value) -> String {
    let mut lines = Vec::new();
    let verdict =
        |value: &Value| if matches!(value.get("matches"), Some(Value::Bool(true))) { "match" } else { "DIFFER" };
    let number = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Float(f)) => format!("{f}"),
        Some(Value::Int(i)) => i.to_string(),
        _ => "-".to_owned(),
    };
    if let Some(Value::Object(layers)) = report.get("layers") {
        for (function, layer) in layers {
            lines.push(format!(
                "{:<8} {function}: significant {} mm2 in {} region(s), raw difference {} mm2",
                verdict(layer),
                number(layer, "significant_area_mm2"),
                number(layer, "significant_regions"),
                number(layer, "difference_area_mm2"),
            ));
        }
    }
    if let Some(part) = report.get("drills") {
        let (a, b) = (number(part, "only_in_reference"), number(part, "only_in_candidate"));
        lines.push(format!(
            "{:<8} drills: {} hits, {a} only in reference, {b} only in candidate",
            verdict(part),
            number(part, "reference_hits")
        ));
    }
    if let Some(part) = report.get("netlist") {
        let (a, b) = (number(part, "only_in_reference"), number(part, "only_in_candidate"));
        let renamed = part.get("renamed_nets").and_then(Value::as_array).map_or(0, <[Value]>::len);
        lines.push(format!(
            "{:<8} netlist: {} records, {a} only in reference, {b} only in candidate, {} connectivity difference(s), {renamed} renamed net(s)",
            verdict(part),
            number(part, "reference_records"),
            number(part, "connectivity_differences"),
        ));
    }
    if let Some(part) = report.get("positions") {
        lines.push(format!("{:<8} positions: {} difference(s)", verdict(part), number(part, "differences")));
    }
    lines.push(format!(
        "CAM comparison: {}",
        if verdict(report) == "match" { "all files match" } else { "differences found" }
    ));
    lines.join("\n")
}

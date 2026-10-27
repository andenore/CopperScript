//! Locked planar DXF guides (`pcbir.mechanical_references`): never board
//! material, copper or manufacturing intent.

use std::io::Read;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};
use sha2::{Digest, Sha256};

use copper_json::{Object, Value as Json, python_isspace};
use copper_physical::Point;
use copper_physical::mechanical::validated_ring;
use copper_physical::pyfloat::round_int;
use copper_units::Decimal;

use super::{Fail, Lower, fail, point};
use crate::paths;
use crate::pyvalue;
use crate::syntax::{MechanicalItemDecl, Scalar, Value};

const MAX_BYTES: u64 = 1_048_576;
const MAX_ENTITIES: usize = 2048;
const MAX_VERTICES: usize = 8192;
const LIMIT: i64 = 1_000_000_000_000;

/// `ReferenceEntity`: one DXF line, circle, arc or polyline.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceEntity {
    pub kind: &'static str,
    pub points: Vec<Point>,
    pub radius_nm: i64,
    pub start_degrees: Decimal,
    pub sweep_degrees: Decimal,
    pub closed: bool,
}

impl ReferenceEntity {
    fn new(
        kind: &'static str,
        points: Vec<Point>,
        radius_nm: i64,
        start_degrees: Decimal,
        sweep_degrees: Decimal,
        closed: bool,
    ) -> Lower<Self> {
        if points.is_empty() {
            return fail("reference entity requires typed points");
        }
        if points.iter().any(|p| p.x_nm.abs() > LIMIT || p.y_nm.abs() > LIMIT) {
            return fail("reference coordinates exceed +/-1km");
        }
        if kind == "line" && (points.len() != 2 || points[0] == points[1]) {
            return fail("reference line requires distinct endpoints");
        }
        if (kind == "circle" || kind == "arc") && (points.len() != 1 || !(0 < radius_nm && radius_nm <= LIMIT)) {
            return fail("reference circle/arc requires a centre and positive bounded radius");
        }
        if kind == "arc" && !(Decimal::zero() < sweep_degrees && sweep_degrees < Decimal::from_i64(360)) {
            return fail("reference arc requires finite angles and sweep between 0 and 360");
        }
        if kind == "polyline" {
            if points.len() < 2 || points.windows(2).any(|pair| pair[0] == pair[1]) {
                return fail("reference polyline requires distinct adjacent vertices");
            }
            if closed {
                if points.len() > 512 {
                    return fail("closed DXF polylines are limited to 512 vertices for bounded topology checks");
                }
                validated_ring(points.clone())?;
            }
        }
        Ok(ReferenceEntity { kind, points, radius_nm, start_degrees, sweep_degrees, closed })
    }
}

/// `MechanicalReference`: a checksum-locked DXF guide and its board pose.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalReference {
    pub id: String,
    pub file: String,
    pub sha256: String,
    pub units: String,
    pub frame: String,
    pub position: Point,
    pub rotation_degrees: Decimal,
    pub mirror_x: bool,
    pub side: String,
    pub purpose: String,
    pub entities: Vec<ReferenceEntity>,
    pub asset_path: String,
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_device_name(part: &str) -> bool {
    let upper = part.to_ascii_uppercase();
    let rest = ["CON", "PRN", "AUX", "NUL"].iter().find_map(|name| upper.strip_prefix(name)).or_else(|| {
        ["COM", "LPT"].iter().find_map(|name| {
            upper.strip_prefix(name).and_then(|rest| rest.strip_prefix(|c: char| ('1'..='9').contains(&c)))
        })
    });
    rest.is_some_and(|rest| rest.is_empty() || (rest.starts_with('.') && !rest.contains('\n')))
}

fn os_error(failure: &std::io::Error, path: &Path) -> Fail {
    Fail::Value(paths::python_os_error(failure, Some(&paths::display(path))))
}

/// `_asset_path`: a relative DXF beneath the declaring source's directory.
fn asset_path(filename: &str, relative: &Value) -> Lower<PathBuf> {
    let relative = match relative {
        Value::Scalar(Scalar::Str(text)) if !text.is_empty() && !text.contains('\\') && !text.contains(':') => text,
        _ => return fail("reference path must be a relative forward-slash DXF path"),
    };
    let raw_parts: Vec<&str> = relative.split('/').collect();
    if raw_parts.iter().any(|part| is_device_name(part)) {
        return fail("reference paths cannot name Windows device files");
    }
    let name = raw_parts.last().copied().unwrap_or_default();
    let suffix = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    };
    if relative.starts_with('/')
        || raw_parts.iter().any(|part| ["", ".", ".."].contains(part) || part.ends_with(' ') || part.ends_with('.'))
        || suffix.to_lowercase() != ".dxf"
    {
        return fail("reference path must stay beneath its declaring source directory");
    }
    if filename.starts_with('<') {
        return fail("reference assets require a source filename");
    }
    let source = std::path::absolute(filename).unwrap_or_else(|_| PathBuf::from(filename));
    let directory = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let target = raw_parts.iter().fold(directory.clone(), |path, part| path.join(part));
    let mut chain: Vec<&Path> = target.ancestors().skip(1).collect();
    chain.push(&target);
    for candidate in chain {
        let info = std::fs::symlink_metadata(candidate)
            .map_err(|failure| Fail::Value(paths::python_os_call_error(&failure, Some(&paths::display(candidate)))))?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            info.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if info.file_type().is_symlink() || reparse {
            return fail("reference assets do not follow symbolic links/junctions");
        }
    }
    if !paths::is_relative_to(&paths::resolve(&target), &paths::resolve(&directory)) {
        return fail("reference asset escapes its source directory");
    }
    Ok(target)
}

/// `read_locked_asset`: the bytes of a checksum-locked asset.
fn read_locked_asset(filename: &str, relative: &Value, digest: &Value) -> Lower<(PathBuf, Vec<u8>)> {
    let digest = match digest {
        Value::Scalar(Scalar::Str(text)) if is_sha256(text) => text,
        _ => return fail("reference requires a lowercase SHA-256 checksum"),
    };
    let target = asset_path(filename, relative)?;
    #[cfg(unix)]
    if target.is_dir() {
        return Err(os_error(&std::io::Error::from_raw_os_error(21), &target));
    }
    let mut stream = std::fs::File::open(&target).map_err(|failure| os_error(&failure, &target))?;
    let info = stream.metadata().map_err(|failure| os_error(&failure, &target))?;
    if !info.is_file() || info.len() > MAX_BYTES {
        return fail("reference asset must be a regular DXF file of at most 1MiB");
    }
    let mut raw = Vec::new();
    (&mut stream).take(MAX_BYTES + 1).read_to_end(&mut raw).map_err(|failure| os_error(&failure, &target))?;
    let actual: String = Sha256::digest(&raw).iter().map(|byte| format!("{byte:02x}")).collect();
    if raw.len() as u64 > MAX_BYTES || actual != *digest {
        return fail("reference asset checksum mismatch or oversized file");
    }
    Ok((target, raw))
}

/// Python's `str.splitlines()` for ASCII text.
fn splitlines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut index) = (0, 0);
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'\n' | b'\r' | 0x0b | 0x0c | 0x1c | 0x1d | 0x1e) {
            lines.push(&text[start..index]);
            index += if byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') { 2 } else { 1 };
            start = index;
        } else {
            index += 1;
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// A DXF group code as Python's `int(text)` reads it; codes beyond `i64`
/// collapse to a value no rule names.
fn group_code(text: &str) -> Option<i64> {
    let text = text.trim_matches(python_isspace);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let bytes = digits.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() || !bytes[bytes.len() - 1].is_ascii_digit() {
        return None;
    }
    if bytes.windows(2).any(|pair| pair == b"__") || bytes.iter().any(|b| !b.is_ascii_digit() && *b != b'_') {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    let value: BigInt = clean.parse().ok()?;
    let value = if negative { -value } else { value };
    Some(value.to_i64().unwrap_or(i64::MIN))
}

fn is_dxf_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        index += 1;
    }
    let digits = |index: &mut usize| {
        let start = *index;
        while *index < bytes.len() && bytes[*index].is_ascii_digit() {
            *index += 1;
        }
        *index - start
    };
    let integer = digits(&mut index);
    if integer > 0 {
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            digits(&mut index);
        }
    } else {
        if bytes.get(index) != Some(&b'.') {
            return false;
        }
        index += 1;
        if digits(&mut index) == 0 {
            return false;
        }
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent = digits(&mut index);
        if !(1..=3).contains(&exponent) {
            return false;
        }
    }
    index == bytes.len()
}

fn number(text: &str) -> Lower<Decimal> {
    if text.len() > 64 || !is_dxf_number(text) {
        return fail("invalid DXF numeric value");
    }
    let value = Decimal::parse(text).map_err(|failure| Fail::Rust(failure.to_string()))?;
    if value.abs() > Decimal::from_i64(LIMIT) {
        return fail("DXF number outside limits");
    }
    Ok(value)
}

/// The exact rational value of a decimal.
fn rational(value: &Decimal) -> BigRational {
    let coefficient = BigInt::from(value.coefficient().clone());
    let coefficient = if value.is_negative() { -coefficient } else { coefficient };
    let exponent = value.exponent();
    let scale = num_traits::pow(BigInt::from(10), exponent.unsigned_abs() as usize);
    if exponent >= 0 { BigRational::from_integer(coefficient * scale) } else { BigRational::new(coefficient, scale) }
}

fn units_factor(units: &str) -> Option<i64> {
    match units {
        "mm" => Some(1_000_000),
        "inch" => Some(25_400_000),
        _ => None,
    }
}

/// `parse_dxf`: the strict ASCII tagged subset.
fn parse_dxf(raw: &[u8], units: &Value) -> Lower<Vec<ReferenceEntity>> {
    let Some(factor) = pyvalue::as_str(units).and_then(units_factor) else {
        return fail("DXF requires explicit mm/inch units and bounded input");
    };
    let units = pyvalue::as_str(units).unwrap_or_default();
    let Ok(text) =
        std::str::from_utf8(raw).map_err(|_| ()).and_then(|text| if text.is_ascii() { Ok(text) } else { Err(()) })
    else {
        return fail("only ASCII DXF reference files are supported");
    };
    let lines = splitlines(text);
    if lines.is_empty() || !lines.len().is_multiple_of(2) {
        return fail("malformed DXF group pairs");
    }
    let mut pairs: Vec<(i64, &str)> = Vec::new();
    for pair in lines.chunks(2) {
        let Some(code) = group_code(pair[0]) else { return fail("invalid DXF group code") };
        pairs.push((code, pair[1].trim_matches(python_isspace)));
    }
    pairs.retain(|pair| pair.0 != 999);
    let mut sections: IndexMap<&str, Vec<(i64, &str)>> = IndexMap::new();
    let mut index = 0;
    let mut ended = false;
    while index < pairs.len() {
        if pairs[index] == (0, "EOF") {
            if index != pairs.len() - 1 {
                return fail("DXF has data after EOF");
            }
            ended = true;
            break;
        }
        if pairs[index] != (0, "SECTION") || index + 1 >= pairs.len() || pairs[index + 1].0 != 2 {
            return fail("DXF requires SECTION records and terminal EOF");
        }
        let name = pairs[index + 1].1;
        index += 2;
        let mut body = Vec::new();
        while index < pairs.len() && pairs[index] != (0, "ENDSEC") {
            if pairs[index] == (0, "SECTION") {
                return fail("nested DXF section");
            }
            body.push(pairs[index]);
            index += 1;
        }
        if index == pairs.len() {
            return fail("unterminated DXF section");
        }
        if sections.contains_key(name) {
            return fail("duplicate DXF section");
        }
        sections.insert(name, body);
        index += 1;
    }
    if !ended {
        return fail("DXF missing EOF");
    }
    let Some(entities_section) = sections.get("ENTITIES") else { return fail("DXF missing ENTITIES") };
    if sections.keys().any(|name| !["HEADER", "TABLES", "ENTITIES", "BLOCKS"].contains(name))
        || sections.get("BLOCKS").is_some_and(|blocks| !blocks.is_empty())
    {
        return fail("DXF blocks/objects/unsupported sections are not permitted");
    }
    let header = sections.get("HEADER").cloned().unwrap_or_default();
    let mut declared = Vec::new();
    for (i, (code, value)) in header.iter().enumerate() {
        if *code == 9 && *value == "$INSUNITS" {
            if i + 1 == header.len() || header[i + 1].0 != 70 {
                return fail("invalid DXF INSUNITS");
            }
            declared.push(header[i + 1].1);
        }
    }
    let expected = if units == "mm" { "4" } else { "1" };
    if declared.len() > 1 || declared.first().is_some_and(|value| *value != "0" && *value != expected) {
        return fail("DXF header units conflict with explicit source units");
    }
    let length = |text: &str| -> Lower<i64> {
        let value = rational(&number(text)?) * BigRational::from_integer(BigInt::from(factor));
        if !value.is_integer() || value.abs() > BigRational::from_integer(BigInt::from(LIMIT)) {
            return fail("DXF lengths must be exact bounded nanometres");
        }
        Ok(value.to_integer().to_i64().expect("bounded"))
    };
    let mut records: Vec<(&str, Vec<(i64, &str)>)> = Vec::new();
    for &(code, value) in entities_section {
        if code == 0 {
            records.push((value, Vec::new()));
        } else if let Some(record) = records.last_mut() {
            record.1.push((code, value));
        } else {
            return fail("DXF entity data precedes entity type");
        }
    }
    if !(1..=MAX_ENTITIES).contains(&records.len()) {
        return fail("DXF requires 1\u{2013}2048 entities");
    }
    let mut entities = Vec::new();
    let mut vertices = 0;
    let common: &[i64] = &[5, 6, 8, 62, 67, 100, 330, 370, 410, 420, 440, 210, 220, 230, 39];
    for (kind, groups) in &records {
        let geometry: &[i64] = match *kind {
            "LINE" => &[10, 20, 30, 11, 21, 31],
            "CIRCLE" => &[10, 20, 30, 40],
            "ARC" => &[10, 20, 30, 40, 50, 51],
            "LWPOLYLINE" => &[90, 70, 43, 38, 10, 20, 40, 41, 42, 91],
            other => return fail(format!("unsupported DXF entity {}", copper_json::python_str_repr(other))),
        };
        if groups.iter().any(|(code, _)| !common.contains(code) && !geometry.contains(code)) {
            return fail("unsupported DXF entity group code");
        }
        let mut values: IndexMap<i64, Vec<&str>> = IndexMap::new();
        for &(code, value) in groups {
            values.entry(code).or_default().push(value);
        }
        let scalar = |code: i64, default: Option<&'static str>| -> Lower<&str> {
            let entries = values.get(&code).map(Vec::as_slice).unwrap_or_default();
            if entries.len() > 1 || (entries.is_empty() && default.is_none()) {
                return fail(format!("missing/duplicate DXF scalar {code}"));
            }
            Ok(entries.first().copied().or(default).unwrap_or_default())
        };
        for (code, default) in [(210, "0"), (220, "0"), (230, "1"), (39, "0"), (30, "0"), (31, "0"), (38, "0")] {
            if number(scalar(code, Some(default))?)? != Decimal::parse(default).expect("literal") {
                return fail("nonplanar DXF/extrusion/thickness is unsupported");
            }
        }
        let entity = if *kind == "LWPOLYLINE" {
            for code in [40, 41, 42, 43] {
                for value in values.get(&code).map(Vec::as_slice).unwrap_or_default() {
                    if !number(value)?.is_zero() {
                        return fail("DXF polyline width/bulge is unsupported");
                    }
                }
            }
            let flags = number(scalar(70, Some("0"))?)?;
            let count = number(scalar(90, None)?)?;
            let flag = flags.to_i64().filter(|_| flags.is_integral());
            let valid_flags = flag.is_some_and(|flag| [0, 1, 128, 129].contains(&flag));
            if !valid_flags
                || !count.is_integral()
                || !(Decimal::from_i64(2) <= count && count <= Decimal::from_i64(MAX_VERTICES as i64))
            {
                return fail("invalid DXF polyline flags/count");
            }
            let mut points = Vec::new();
            let mut x = None;
            for &(code, value) in groups {
                if code == 10 {
                    if x.is_some() {
                        return fail("DXF polyline vertex is missing Y");
                    }
                    x = Some(length(value)?);
                } else if code == 20 {
                    let Some(first) = x.take() else { return fail("DXF polyline vertex is missing X") };
                    points.push(Point::new(first, length(value)?));
                }
            }
            if x.is_some() || Decimal::from_i64(points.len() as i64) != count {
                return fail("DXF polyline vertex count mismatch");
            }
            let closed = flag.unwrap_or_default() & 1 != 0;
            ReferenceEntity::new("polyline", points, 0, Decimal::zero(), Decimal::zero(), closed)?
        } else {
            let start = Point::new(length(scalar(10, None)?)?, length(scalar(20, None)?)?);
            match *kind {
                "LINE" => {
                    let end = Point::new(length(scalar(11, None)?)?, length(scalar(21, None)?)?);
                    ReferenceEntity::new("line", vec![start, end], 0, Decimal::zero(), Decimal::zero(), false)?
                }
                "CIRCLE" => {
                    let radius = length(scalar(40, None)?)?;
                    ReferenceEntity::new("circle", vec![start], radius, Decimal::zero(), Decimal::zero(), false)?
                }
                _ => {
                    let (begin, end) = (number(scalar(50, None)?)?, number(scalar(51, None)?)?);
                    let full = Decimal::from_i64(360);
                    let rust = |failure: copper_units::DecimalError| Fail::Rust(failure.to_string());
                    let mut sweep = end.sub(&begin).and_then(|value| value.rem(&full)).map_err(rust)?;
                    if sweep < Decimal::zero() {
                        sweep = sweep.add(&full).map_err(rust)?;
                    }
                    let radius = length(scalar(40, None)?)?;
                    ReferenceEntity::new("arc", vec![start], radius, begin, sweep, false)?
                }
            }
        };
        vertices += entity.points.len();
        if vertices > MAX_VERTICES {
            return fail("DXF reference exceeds 8192 vertices");
        }
        entities.push(entity);
    }
    Ok(entities)
}

/// `lower_reference`.
pub fn lower_reference(item: &MechanicalItemDecl) -> Lower<MechanicalReference> {
    let p = &item.parameters;
    let required = ["file", "sha256", "units", "frame"];
    let allowed = ["file", "sha256", "units", "frame", "position", "rotation", "mirror_x", "side", "purpose"];
    if item.shape != "dxf"
        || p.keys().any(|key| !allowed.contains(&key.as_str()))
        || !required.iter().all(|key| p.contains_key(*key))
    {
        return fail("reference dxf requires file, sha256, units and frame with optional transform/side/purpose");
    }
    let angle = match p.get("rotation") {
        None => Some(Decimal::zero()),
        Some(Value::Scalar(scalar @ (Scalar::Int(_) | Scalar::Float(_)))) => pyvalue::decimal(scalar),
        Some(_) => None,
    };
    let Some(angle) = angle else { return fail("reference rotation must be finite numeric degrees") };
    let (target, raw) = read_locked_asset(&item.location.filename, &p["file"], &p["sha256"])?;
    let position = p.get("position").map(point).transpose()?.unwrap_or(Point::new(0, 0));
    let rotation = angle.rem(&Decimal::from_i64(360)).map_err(|failure| Fail::Rust(failure.to_string()))?;
    let entities = parse_dxf(&raw, &p["units"])?;
    let text = |key: &str| pyvalue::as_str(&p[key]).unwrap_or_default().to_owned();
    let frame = pyvalue::as_str(&p["frame"]).filter(|frame| ["cartesian", "board"].contains(frame));
    if item.name.is_empty() || frame.is_none() {
        return fail("reference requires explicit mm/inch units and cartesian/board frame");
    }
    let side = match p.get("side") {
        None => Some("both"),
        Some(value) => pyvalue::as_str(value).filter(|side| ["front", "back", "both"].contains(side)),
    };
    let mirror = match p.get("mirror_x") {
        None => Some(false),
        Some(Value::Scalar(Scalar::Bool(flag))) => Some(*flag),
        Some(_) => None,
    };
    let (Some(side), Some(mirror_x)) = (side, mirror) else { return fail("reference side/mirror is invalid") };
    let purpose = match p.get("purpose") {
        None => Some("Reference guide only".to_owned()),
        Some(Value::Scalar(Scalar::Str(text))) if text.chars().count() <= 4096 => Some(text.clone()),
        Some(_) => None,
    };
    let Some(purpose) = purpose else { return fail("reference file/purpose must be bounded strings") };
    if position.x_nm.abs() > LIMIT || position.y_nm.abs() > LIMIT {
        return fail("reference position exceeds +/-1km");
    }
    Ok(MechanicalReference {
        id: item.name.clone(),
        file: text("file"),
        sha256: text("sha256"),
        units: text("units"),
        frame: frame.unwrap_or_default().to_owned(),
        position,
        rotation_degrees: rotation,
        mirror_x,
        side: side.to_owned(),
        purpose,
        entities,
        asset_path: paths::display(&target),
    })
}

/// `validate_reference_inventory`.
pub fn validate_reference_inventory(references: &[MechanicalReference]) -> Lower<()> {
    let entities: usize = references.iter().map(|r| r.entities.len()).sum();
    let points: usize = references.iter().flat_map(|r| &r.entities).map(|e| e.points.len()).sum();
    if references.len() > 32 || entities > 8192 || points > 16384 {
        return fail("reference inventory exceeds 32 assets, 8192 entities or 16384 vertices");
    }
    Ok(())
}

fn decimal_float(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

/// Round half to even, exactly.
fn round_rational(value: &BigRational) -> BigInt {
    let floor = value.floor().to_integer();
    let fraction = value - BigRational::from_integer(floor.clone());
    let half = BigRational::new(BigInt::one(), BigInt::from(2));
    if fraction > half || (fraction == half && (&floor % 2u8) != BigInt::zero()) { floor + 1 } else { floor }
}

/// `reference_scene`: read-only board-frame guides in nanometres.
pub fn reference_scene(reference: &MechanicalReference) -> Json {
    let full = Decimal::from_i64(360);
    let mut angle = reference.rotation_degrees.rem(&full).expect("finite rotation");
    if angle < Decimal::zero() {
        angle = angle.add(&full).expect("finite rotation");
    }
    let quarter = |degrees: i64| angle == Decimal::from_i64(degrees);
    let (cs, sn) = if quarter(0) {
        (Decimal::from_i64(1), Decimal::zero())
    } else if quarter(90) {
        (Decimal::zero(), Decimal::from_i64(1))
    } else if quarter(180) {
        (Decimal::from_i64(-1), Decimal::zero())
    } else if quarter(270) {
        (Decimal::zero(), Decimal::from_i64(-1))
    } else {
        let radians = decimal_float(&angle).to_radians();
        let decimal = |value: f64| Decimal::parse(&copper_json::python_float_repr(value)).expect("finite float");
        (decimal(radians.cos()), decimal(radians.sin()))
    };
    let (c, s) = (rational(&cs), rational(&sn));
    let transform = |p: Point| -> Json {
        let x = BigRational::from_integer(BigInt::from(if reference.mirror_x { -p.x_nm } else { p.x_nm }));
        let y = BigRational::from_integer(BigInt::from(if reference.frame == "cartesian" { -p.y_nm } else { p.y_nm }));
        let tx = round_rational(&(&x * &c + &y * &s)) + reference.position.x_nm;
        let ty = round_rational(&(-&x * &s + &y * &c)) + reference.position.y_nm;
        Json::Array(vec![Json::Int(tx), Json::Int(ty)])
    };
    let entities = reference
        .entities
        .iter()
        .map(|entity| {
            let mut record = Object::new();
            record.insert("kind".to_owned(), Json::from(entity.kind));
            record.insert("points".to_owned(), Json::Array(entity.points.iter().map(|p| transform(*p)).collect()));
            record.insert("radius_nm".to_owned(), Json::from(entity.radius_nm));
            record.insert("closed".to_owned(), Json::Bool(entity.closed));
            if entity.kind == "arc" {
                let center = entity.points[0];
                let endpoint = |degrees: &Decimal| {
                    let a = decimal_float(degrees).to_radians();
                    let radius = entity.radius_nm as f64;
                    transform(Point::new(
                        round_int(center.x_nm as f64 + radius * a.cos()),
                        round_int(center.y_nm as f64 + radius * a.sin()),
                    ))
                };
                let finish = entity.start_degrees.add(&entity.sweep_degrees).expect("bounded angles");
                record.insert("start".to_owned(), endpoint(&entity.start_degrees));
                record.insert("end".to_owned(), endpoint(&finish));
                record.insert("large".to_owned(), Json::Bool(entity.sweep_degrees > Decimal::from_i64(180)));
                record.insert("sweep".to_owned(), Json::Bool(!((reference.frame == "cartesian") ^ reference.mirror_x)));
            }
            Json::Object(record)
        })
        .collect();
    let mut scene = Object::new();
    scene.insert("id".to_owned(), Json::from(reference.id.as_str()));
    scene.insert("sha256".to_owned(), Json::from(reference.sha256.as_str()));
    scene.insert("file".to_owned(), Json::from(reference.file.as_str()));
    scene.insert("units".to_owned(), Json::from(reference.units.as_str()));
    scene.insert("frame".to_owned(), Json::from(reference.frame.as_str()));
    scene.insert(
        "position".to_owned(),
        Json::Array(vec![Json::from(reference.position.x_nm), Json::from(reference.position.y_nm)]),
    );
    scene.insert("rotation".to_owned(), Json::from(reference.rotation_degrees.to_sci_string().as_str()));
    scene.insert("mirror_x".to_owned(), Json::Bool(reference.mirror_x));
    scene.insert("side".to_owned(), Json::from(reference.side.as_str()));
    scene.insert("purpose".to_owned(), Json::from(reference.purpose.as_str()));
    scene.insert("read_only".to_owned(), Json::Bool(true));
    scene.insert("entities".to_owned(), Json::Array(entities));
    Json::Object(scene)
}

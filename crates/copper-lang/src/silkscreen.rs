//! Board artwork intent (`pcbir.silkscreen`): the `silkscreen` block, the
//! branding mark and the artwork written into the compiled IR.

use std::sync::OnceLock;

use copper_json::{Object, Value as Json, python_str_repr as repr};
use copper_physical::mechanical::validated_ring;
use copper_physical::pyfloat::round_int;
use copper_physical::{BoardSide, Point};
use copper_units::Decimal;

use crate::error::CopperScriptError;
use crate::mechanical::{Fail, Lower, board_side, coordinate, fail, nanometres, strip, text_enum};
use crate::pyvalue;
use crate::syntax::{Declaration, Document, Scalar, SilkscreenItemDecl, Value};

/// `ArtworkMaterial`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtworkMaterial {
    Silkscreen,
    Copper,
}

impl ArtworkMaterial {
    pub fn value(self) -> &'static str {
        match self {
            ArtworkMaterial::Silkscreen => "silkscreen",
            ArtworkMaterial::Copper => "copper",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "silkscreen" => Some(ArtworkMaterial::Silkscreen),
            "copper" => Some(ArtworkMaterial::Copper),
            _ => None,
        }
    }
}

/// Footprint graphics on the silkscreen layer (`physical.Footprint*`).
#[derive(Clone, Debug, PartialEq)]
pub enum Graphic {
    Line { start: Point, end: Point, width_nm: i64 },
    Rectangle { start: Point, end: Point, width_nm: i64, filled: bool },
    Circle { center: Point, end: Point, width_nm: i64, filled: bool },
    Arc { start: Point, midpoint: Point, end: Point, width_nm: i64 },
    Polygon { points: Vec<Point>, width_nm: i64, filled: bool },
}

impl Graphic {
    fn width_nm(&self) -> i64 {
        match self {
            Graphic::Line { width_nm, .. }
            | Graphic::Rectangle { width_nm, .. }
            | Graphic::Circle { width_nm, .. }
            | Graphic::Arc { width_nm, .. }
            | Graphic::Polygon { width_nm, .. } => *width_nm,
        }
    }

    fn filled(&self) -> bool {
        match self {
            Graphic::Rectangle { filled, .. } | Graphic::Circle { filled, .. } | Graphic::Polygon { filled, .. } => {
                *filled
            }
            Graphic::Line { .. } | Graphic::Arc { .. } => false,
        }
    }

    fn class_name(&self) -> &'static str {
        match self {
            Graphic::Line { .. } => "FootprintLine",
            Graphic::Rectangle { .. } => "FootprintRectangle",
            Graphic::Circle { .. } => "FootprintCircle",
            Graphic::Arc { .. } => "FootprintArc",
            Graphic::Polygon { .. } => "FootprintPolygon",
        }
    }
}

/// One artwork item: `SilkscreenText` or `SilkscreenGraphic`.
#[derive(Clone, Debug, PartialEq)]
pub enum Artwork {
    Text {
        id: String,
        text: String,
        position: Point,
        size_nm: i64,
        thickness_nm: i64,
        rotation_degrees: Decimal,
        side: BoardSide,
        material: ArtworkMaterial,
        exposed: bool,
    },
    Graphic {
        id: String,
        graphic: Graphic,
        side: BoardSide,
        material: ArtworkMaterial,
        exposed: bool,
    },
}

impl Artwork {
    pub fn id(&self) -> &str {
        match self {
            Artwork::Text { id, .. } | Artwork::Graphic { id, .. } => id,
        }
    }
}

/// `SilkscreenBranding`: the Cu mark and compiler version label.
#[derive(Clone, Debug, PartialEq)]
pub struct SilkscreenBranding {
    pub enabled: bool,
    pub position: Option<Point>,
    pub size_nm: i64,
    pub rotation_degrees: Decimal,
    pub side: BoardSide,
    pub version: String,
    pub material: ArtworkMaterial,
    pub exposed: bool,
    pub layout: String,
}

impl Default for SilkscreenBranding {
    fn default() -> Self {
        SilkscreenBranding {
            enabled: true,
            position: None,
            size_nm: 5_000_000,
            rotation_degrees: Decimal::zero(),
            side: BoardSide::Front,
            version: compiler_version(),
            material: ArtworkMaterial::Silkscreen,
            exposed: false,
            layout: "horizontal".to_owned(),
        }
    }
}

/// `compiler_version()`: the installed package version.
pub fn compiler_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SilkscreenDesign {
    pub branding: SilkscreenBranding,
    pub artwork: Vec<Artwork>,
}

const WHAT: &str = "silkscreen";

fn length(value: &Value) -> Lower<i64> {
    nanometres(value, WHAT)
}

fn point(value: &Value) -> Lower<Point> {
    coordinate(value, WHAT)
}

fn check_material(material: ArtworkMaterial, exposed: &Value) -> Lower<bool> {
    let Value::Scalar(Scalar::Bool(exposed)) = exposed else { return fail("exposed must be boolean") };
    if *exposed && material != ArtworkMaterial::Copper {
        return fail("exposed requires material = copper");
    }
    Ok(*exposed)
}

fn positive(value: i64, label: &str) -> Lower<()> {
    if value <= 0 {
        return fail(format!("{label} must be positive integer nanometres"));
    }
    Ok(())
}

/// `_rotation`: finite degrees modulo 360.
fn rotation(value: &Scalar) -> Lower<Decimal> {
    let Some(degrees) = pyvalue::decimal(value) else { return fail("rotation must be finite degrees") };
    degrees.rem(&Decimal::from_i64(360)).map_err(|failure| Fail::Value(failure.to_string()))
}

/// Python's `tuple(map(_point, value))`.
fn iterated_points(value: &Value) -> Lower<Vec<Point>> {
    match value {
        Value::Points(points) => points.iter().map(|(x, y)| point(&Value::Point(x.clone(), y.clone()))).collect(),
        Value::Point(..) => fail("expected a coordinate pair (x, y)"),
        Value::Scalar(Scalar::Str(text)) if text.is_empty() => Ok(Vec::new()),
        Value::Scalar(Scalar::Str(_)) => fail("expected a coordinate pair (x, y)"),
        other => fail(format!("'{}' object is not iterable", pyvalue::type_name(other))),
    }
}

fn graphic(id: &str, graphic: Graphic, side: BoardSide, material: ArtworkMaterial, exposed: bool) -> Lower<Artwork> {
    if graphic.width_nm() == 0 && !graphic.filled() {
        return fail("unfilled silkscreen graphics require positive thickness");
    }
    Ok(Artwork::Graphic { id: id.to_owned(), graphic, side, material, exposed })
}

fn item_artwork(item: &SilkscreenItemDecl, branding: &mut SilkscreenBranding, artwork: &mut Vec<Artwork>) -> Lower<()> {
    let p = &item.parameters;
    let common = ["side", "material", "exposed"];
    let (properties, required): (&[&str], &[&str]) = match item.kind.as_str() {
        "branding" => (&["enabled", "position", "size", "rotation", "layout"], &[]),
        "logo" => (&["position", "size", "rotation"], &["position"]),
        "text" => (&["text", "position", "size", "thickness", "rotation"], &["position", "text"]),
        "line" => (&["start", "end", "thickness"], &["end", "start"]),
        "rectangle" => (&["start", "end", "thickness", "filled"], &["end", "start"]),
        "circle" => (&["center", "radius", "thickness", "filled"], &["center", "radius"]),
        "arc" => (&["start", "mid", "end", "thickness"], &["end", "mid", "start"]),
        _ => (&["vertices", "thickness", "filled"], &["vertices"]),
    };
    let known = |key: &str| properties.contains(&key) || common.contains(&key);
    if let Some(key) = p.keys().filter(|key| !known(key)).min() {
        return fail(format!("unknown silkscreen property {}", repr(key)));
    }
    if let Some(key) = required.iter().find(|key| !p.contains_key(**key)) {
        return fail(format!("missing silkscreen property {}", repr(key)));
    }
    let text = |value: &str| Value::Scalar(Scalar::Str(value.to_owned()));
    let side = board_side(p.get("side").unwrap_or(&text("front")))?;
    let material =
        text_enum(p.get("material").unwrap_or(&text("silkscreen")), "ArtworkMaterial", ArtworkMaterial::parse)?;
    let exposed = check_material(material, p.get("exposed").unwrap_or(&Value::Scalar(Scalar::Bool(false))))?;
    let degrees = match p.get("rotation") {
        None => Scalar::Int(0.into()),
        Some(Value::Scalar(scalar @ (Scalar::Int(_) | Scalar::Float(_)))) => scalar.clone(),
        Some(_) => return fail("rotation requires a number in degrees"),
    };
    if item.kind == "branding" || item.kind == "logo" {
        let position = p.get("position").map(point).transpose()?;
        let size = p.get("size").map(length).transpose()?.unwrap_or(5_000_000);
        let enabled = match p.get("enabled") {
            None => true,
            Some(Value::Scalar(Scalar::Bool(flag))) => *flag,
            Some(_) => return fail("branding enabled must be boolean"),
        };
        positive(size, "logo size")?;
        let layout = match p.get("layout") {
            None => "horizontal",
            Some(Value::Scalar(Scalar::Str(layout))) if layout == "horizontal" || layout == "stacked" => layout,
            Some(_) => return fail("branding layout must be horizontal or stacked"),
        };
        let rotation = rotation(&degrees)?;
        if item.kind == "branding" {
            *branding = SilkscreenBranding {
                enabled,
                position,
                size_nm: size,
                rotation_degrees: rotation,
                side,
                version: compiler_version(),
                material,
                exposed,
                layout: layout.to_owned(),
            };
        } else {
            let position = position.expect("logo requires a position");
            artwork.extend(logo(&item.name, position, size, &rotation, side, material, exposed)?);
        }
        return Ok(());
    }
    let width = p.get("thickness").map(length).transpose()?.unwrap_or(150_000);
    if item.kind == "text" {
        let position = point(&p["position"])?;
        let size = p.get("size").map(length).transpose()?.unwrap_or(1_000_000);
        if strip(&item.name).is_empty() {
            return fail("silkscreen requires an id");
        }
        let content = match &p["text"] {
            Value::Scalar(Scalar::Str(content)) if !strip(content).is_empty() => content.clone(),
            _ => return fail("silkscreen text requires a position and nonempty text"),
        };
        if content.chars().any(|c| (c as u32) < 32 && c != '\n') {
            return fail("silkscreen text contains unsupported control characters");
        }
        positive(size, "text size")?;
        positive(width, "text thickness")?;
        if i128::from(width) * 2 > i128::from(size) {
            return fail("text thickness must not exceed half its size");
        }
        artwork.push(Artwork::Text {
            id: item.name.clone(),
            text: content,
            position,
            size_nm: size,
            thickness_nm: width,
            rotation_degrees: rotation(&degrees)?,
            side,
            material,
            exposed,
        });
        return Ok(());
    }
    let filled = match p.get("filled") {
        None => false,
        Some(Value::Scalar(Scalar::Bool(flag))) => *flag,
        Some(_) => return fail("filled must be boolean"),
    };
    let check_width = |name: &str| {
        if width < 0 { fail(format!("footprint {name} width cannot be negative")) } else { Ok(()) }
    };
    let shape = match item.kind.as_str() {
        "line" => {
            let (start, end) = (point(&p["start"])?, point(&p["end"])?);
            if start == end {
                return fail("footprint line cannot have zero length");
            }
            check_width("line")?;
            Graphic::Line { start, end, width_nm: width }
        }
        "rectangle" => {
            let (start, end) = (point(&p["start"])?, point(&p["end"])?);
            if start.x_nm == end.x_nm || start.y_nm == end.y_nm {
                return fail("silkscreen rectangle requires positive area");
            }
            check_width("rectangle")?;
            Graphic::Rectangle { start, end, width_nm: width, filled }
        }
        "circle" => {
            let (center, radius) = (point(&p["center"])?, length(&p["radius"])?);
            positive(radius, "circle radius")?;
            check_width("circle")?;
            Graphic::Circle { center, end: Point::new(center.x_nm + radius, center.y_nm), width_nm: width, filled }
        }
        "arc" => {
            let (start, mid, end) = (point(&p["start"])?, point(&p["mid"])?, point(&p["end"])?);
            let cross = i128::from(mid.x_nm - start.x_nm) * i128::from(end.y_nm - start.y_nm)
                - i128::from(mid.y_nm - start.y_nm) * i128::from(end.x_nm - start.x_nm);
            if cross == 0 {
                return fail("silkscreen arc points must not be collinear");
            }
            check_width("arc")?;
            Graphic::Arc { start, midpoint: mid, end, width_nm: width }
        }
        _ => {
            let points = validated_ring(iterated_points(&p["vertices"])?)?;
            check_width("polygon")?;
            Graphic::Polygon { points, width_nm: width, filled }
        }
    };
    if strip(&item.name).is_empty() {
        return fail("silkscreen requires an id");
    }
    artwork.push(graphic(&item.name, shape, side, material, exposed)?);
    Ok(())
}

/// `lower_silkscreen`.
pub fn lower_silkscreen(document: &Document) -> Result<SilkscreenDesign, CopperScriptError> {
    let blocks: Vec<_> = document
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Silkscreen(block) => Some(block),
            _ => None,
        })
        .collect();
    if blocks.len() > 1 {
        return Err(CopperScriptError::new(
            "SLK001",
            "only one silkscreen block is allowed",
            blocks[1].location.clone(),
        ));
    }
    let Some(block) = blocks.first() else { return Ok(SilkscreenDesign::default()) };
    let mut branding = SilkscreenBranding::default();
    let mut artwork = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for item in &block.items {
        let lowered = if ids.contains(&item.name.as_str()) || item.name.starts_with("__copperscript") {
            fail(format!("duplicate or reserved silkscreen feature id {}", repr(&item.name)))
        } else {
            ids.push(&item.name);
            item_artwork(item, &mut branding, &mut artwork)
        };
        lowered.map_err(|failure| failure.error("SLK002", &item.location))?;
    }
    Ok(SilkscreenDesign { branding, artwork })
}

/// `_cu_paths`: the bundled mark's closed M/L/C/Z paths, flattened in its
/// 42-unit viewbox (each cubic in eight exact decimal steps).
fn cu_paths() -> &'static Vec<Vec<(Decimal, Decimal)>> {
    static PATHS: OnceLock<Vec<Vec<(Decimal, Decimal)>>> = OnceLock::new();
    PATHS.get_or_init(|| {
        let svg = include_str!("../assets/copper-logo.svg");
        let mut paths = Vec::new();
        for element in svg.split("<path").skip(1) {
            let data = element.split(" d=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("path data");
            let tokens = path_tokens(data);
            let number = |index: &mut usize| {
                *index += 1;
                Decimal::parse(&tokens[*index - 1]).expect("decimal coordinate")
            };
            let mut points: Vec<(Decimal, Decimal)> = Vec::new();
            let mut index = 0;
            while index < tokens.len() {
                index += 1;
                match tokens[index - 1].as_str() {
                    "M" | "L" => {
                        let x = number(&mut index);
                        points.push((x, number(&mut index)));
                    }
                    "C" => {
                        let p0 = points.last().cloned().expect("current point");
                        let mut control = Vec::new();
                        for _ in 0..3 {
                            let x = number(&mut index);
                            control.push((x, number(&mut index)));
                        }
                        let (p1, p2, p3) = (&control[0], &control[1], &control[2]);
                        for step in 1..=8 {
                            let t = Decimal::from_i64(step).div(&Decimal::from_i64(8)).expect("eighths");
                            let x = bezier(&t, &p0.0, &p1.0, &p2.0, &p3.0);
                            points.push((x, bezier(&t, &p0.1, &p1.1, &p2.1, &p3.1)));
                        }
                    }
                    "Z" => paths.push(std::mem::take(&mut points)),
                    _ => panic!("unsupported bundled logo path"),
                }
            }
        }
        paths
    })
}

/// Tokens of `re.findall(r"[MLCZ]|-?\d+(?:\.\d+)?", d)`.
fn path_tokens(data: &str) -> Vec<String> {
    let bytes = data.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'M' | b'L' | b'C' | b'Z') {
            tokens.push((byte as char).to_string());
            index += 1;
        } else if byte.is_ascii_digit() || (byte == b'-' && bytes.get(index + 1).is_some_and(u8::is_ascii_digit)) {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if bytes.get(index) == Some(&b'.') && bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    index += 1;
                }
            }
            tokens.push(data[start..index].to_owned());
        } else {
            index += 1;
        }
    }
    tokens
}

/// `(1-t)**3*p0 + 3*(1-t)**2*t*p1 + 3*(1-t)*t*t*p2 + t**3*p3`, in Python's order.
fn bezier(t: &Decimal, p0: &Decimal, p1: &Decimal, p2: &Decimal, p3: &Decimal) -> Decimal {
    let d = |value: Result<Decimal, copper_units::DecimalError>| value.expect("exact logo arithmetic");
    let one = Decimal::from_i64(1);
    let three = Decimal::from_i64(3);
    let u = d(one.sub(t));
    let cube = |x: &Decimal| d(d(x.mul(x)).mul(x));
    let first = d(cube(&u).mul(p0));
    let second = d(d(d(three.mul(&d(u.mul(&u)))).mul(t)).mul(p1));
    let third = d(d(d(d(three.mul(&u)).mul(t)).mul(t)).mul(p2));
    let fourth = d(cube(t).mul(p3));
    d(d(d(first.add(&second)).add(&third)).add(&fourth))
}

fn decimal_float(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

/// `_transform`: a mark-relative point scaled, mirrored on the back and rotated.
fn transform(
    point: (&Decimal, &Decimal),
    position: Point,
    size: i64,
    rotation: &Decimal,
    side: BoardSide,
) -> Lower<Point> {
    let rust = |failure: copper_units::DecimalError| Fail::Rust(failure.to_string());
    let size = Decimal::from_i64(size);
    let scale = |value: &Decimal| value.mul(&size).and_then(|value| value.div(&Decimal::from_i64(42))).map_err(rust);
    let (mut x, y) = (scale(point.0)?, scale(point.1)?);
    if side == BoardSide::Back {
        x = x.neg().map_err(rust)?;
    }
    let angle = decimal_float(rotation).to_radians();
    let (fx, fy) = (decimal_float(&x), decimal_float(&y));
    Ok(Point::new(
        position.x_nm + round_int(fx * angle.cos() - fy * angle.sin()),
        position.y_nm + round_int(fx * angle.sin() + fy * angle.cos()),
    ))
}

/// `_logo`: the mark as a frame, the Cu glyph polygons and the "29" label.
fn logo(
    identity: &str,
    position: Point,
    size: i64,
    rotation: &Decimal,
    side: BoardSide,
    material: ArtworkMaterial,
    exposed: bool,
) -> Lower<Vec<Artwork>> {
    let rust = |failure: copper_units::DecimalError| Fail::Rust(failure.to_string());
    let half = Decimal::from_i64(21);
    let at = |x: &Decimal, y: &Decimal| -> Lower<Point> {
        let (x, y) = (x.sub(&half).map_err(rust)?, y.sub(&half).map_err(rust)?);
        transform((&x, &y), position, size, rotation, side)
    };
    let frame_points = [
        ("3", "0.5"),
        ("39", "0.5"),
        ("41.5", "3"),
        ("41.5", "39"),
        ("39", "41.5"),
        ("3", "41.5"),
        ("0.5", "39"),
        ("0.5", "3"),
    ];
    let parse = |text: &str| Decimal::parse(text).expect("literal");
    let frame = frame_points.iter().map(|(x, y)| at(&parse(x), &parse(y))).collect::<Lower<Vec<Point>>>()?;
    let mut artwork = vec![graphic(
        &format!("{identity}:frame"),
        Graphic::Polygon { points: frame, width_nm: 1.max(size * 3 / 100), filled: false },
        side,
        material,
        exposed,
    )?];
    for (index, path) in cu_paths().iter().enumerate() {
        let points = path.iter().map(|(x, y)| at(x, y)).collect::<Lower<Vec<Point>>>()?;
        artwork.push(graphic(
            &format!("{identity}:cu:{index}"),
            Graphic::Polygon { points, width_nm: 0, filled: true },
            side,
            material,
            exposed,
        )?);
    }
    let eight = Decimal::from_i64(8);
    artwork.push(Artwork::Text {
        id: format!("{identity}:29"),
        text: "29".to_owned(),
        position: at(&eight, &eight)?,
        size_nm: 1.max(size * 16 / 100),
        thickness_nm: 1.max(size * 2 / 100),
        rotation_degrees: rotation.clone(),
        side,
        material,
        exposed,
    });
    Ok(artwork)
}

fn point_json(point: &Point) -> Json {
    Json::Object(Object::from([
        ("x_nm".to_owned(), Json::from(point.x_nm)),
        ("y_nm".to_owned(), Json::from(point.y_nm)),
    ]))
}

fn graphic_json(graphic: &Graphic) -> Json {
    let mut entries: Vec<(&str, Json)> = match graphic {
        Graphic::Line { start, end, .. } => vec![("start", point_json(start)), ("end", point_json(end))],
        Graphic::Rectangle { start, end, filled, .. } => {
            vec![("start", point_json(start)), ("end", point_json(end)), ("filled", Json::Bool(*filled))]
        }
        Graphic::Circle { center, end, filled, .. } => {
            vec![("center", point_json(center)), ("end", point_json(end)), ("filled", Json::Bool(*filled))]
        }
        Graphic::Arc { start, midpoint, end, .. } => {
            vec![("start", point_json(start)), ("midpoint", point_json(midpoint)), ("end", point_json(end))]
        }
        Graphic::Polygon { points, filled, .. } => {
            vec![("points", Json::Array(points.iter().map(point_json).collect())), ("filled", Json::Bool(*filled))]
        }
    };
    entries.push(("width_nm", Json::from(graphic.width_nm())));
    entries.push(("layer", Json::from("silkscreen")));
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// `artwork_document`: artwork records sorted by id.
pub fn artwork_document(artwork: &[Artwork]) -> Json {
    let mut sorted: Vec<&Artwork> = artwork.iter().collect();
    sorted.sort_by(|a, b| a.id().cmp(b.id()));
    Json::Array(
        sorted
            .into_iter()
            .map(|item| {
                let mut record = Object::new();
                match item {
                    Artwork::Text {
                        id,
                        text,
                        position,
                        size_nm,
                        thickness_nm,
                        rotation_degrees,
                        side,
                        material,
                        exposed,
                    } => {
                        record.insert("kind".to_owned(), Json::from("text"));
                        record.insert("id".to_owned(), Json::from(id.as_str()));
                        record.insert("text".to_owned(), Json::from(text.as_str()));
                        record.insert("position".to_owned(), point_json(position));
                        record.insert("size_nm".to_owned(), Json::from(*size_nm));
                        record.insert("thickness_nm".to_owned(), Json::from(*thickness_nm));
                        record.insert(
                            "rotation_degrees".to_owned(),
                            Json::from(rotation_degrees.decimal_text().as_str()),
                        );
                        record.insert("side".to_owned(), Json::from(side.value()));
                        record.insert("material".to_owned(), Json::from(material.value()));
                        record.insert("exposed".to_owned(), Json::Bool(*exposed));
                    }
                    Artwork::Graphic { id, graphic, side, material, exposed } => {
                        record.insert("kind".to_owned(), Json::from(graphic.class_name()));
                        record.insert("id".to_owned(), Json::from(id.as_str()));
                        record.insert("graphic".to_owned(), graphic_json(graphic));
                        record.insert("side".to_owned(), Json::from(side.value()));
                        record.insert("material".to_owned(), Json::from(material.value()));
                        record.insert("exposed".to_owned(), Json::Bool(*exposed));
                    }
                }
                Json::Object(record)
            })
            .collect(),
    )
}

/// `design_document`: the artwork intent written into the compiled IR.
pub fn design_document(design: &SilkscreenDesign) -> Json {
    let mark = &design.branding;
    let branding = Object::from([
        ("enabled".to_owned(), Json::Bool(mark.enabled)),
        ("position".to_owned(), mark.position.as_ref().map_or(Json::Null, point_json)),
        ("size_nm".to_owned(), Json::from(mark.size_nm)),
        ("rotation_degrees".to_owned(), Json::from(mark.rotation_degrees.to_sci_string().as_str())),
        ("side".to_owned(), Json::from(mark.side.value())),
        ("version".to_owned(), Json::from(mark.version.as_str())),
        ("material".to_owned(), Json::from(mark.material.value())),
        ("exposed".to_owned(), Json::Bool(mark.exposed)),
        ("layout".to_owned(), Json::from(mark.layout.as_str())),
    ]);
    Json::Object(Object::from([
        ("branding".to_owned(), Json::Object(branding)),
        ("artwork".to_owned(), artwork_document(&design.artwork)),
    ]))
}

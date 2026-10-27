//! Import KiCad `.kicad_mod` files (`pcbir.importers.kicad_mod`).
//!
//! A conservative subset of KiCad 6 to 10 footprints. Fabrication-relevant
//! constructs that cannot be represented are errors; presentation-only items
//! are warnings, which `strict` promotes to errors.

use std::fmt;
use std::path::Path;

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use sha2::{Digest, Sha256};

use copper_json::{python_isspace, python_str_repr as repr};
use copper_units::{Decimal, Rounding};

use crate::board::{CopperKeepout, CopperLayer, polygon_ring};
use crate::footprint::{
    FootprintGraphic, FootprintLayer, FootprintPad, InternalPadGroup, PadKind, PadShape, PhysicalFootprint, Size,
    ZoneConnection, merge_internal_pad_groups,
};
use crate::geometry::Point;
use crate::pyfloat::round_int;

/// A failed import: `KiCadModImportError`, or a dataclass `ValueError` that
/// Python lets escape unwrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    Import(String),
    Value(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Import(message) | ImportError::Value(message) => formatter.write_str(message),
        }
    }
}

type Result<T> = std::result::Result<T, ImportError>;

fn import<T>(message: String) -> Result<T> {
    Err(ImportError::Import(message))
}

fn value(message: String) -> ImportError {
    ImportError::Value(message)
}

/// `FootprintImportResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintImportResult {
    pub footprint: PhysicalFootprint,
    pub warnings: Vec<String>,
    pub source_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Atom(String),
    List(Vec<Node>),
}

type List = Vec<Node>;

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Python's message for a failed `bytes.decode("utf-8")`.
fn utf8_error(data: &[u8], error: std::str::Utf8Error) -> String {
    let start = error.valid_up_to();
    let count = error.error_len().unwrap_or(data.len() - start);
    let reason = match error.error_len() {
        None => "unexpected end of data",
        Some(_) if (0xc2..=0xf4).contains(&data[start]) => "invalid continuation byte",
        Some(_) => "invalid start byte",
    };
    let what = if count > 1 {
        format!("bytes in position {start}-{}", start + count - 1)
    } else {
        format!("byte 0x{:02x} in position {start}", data[start])
    };
    format!("'utf-8' codec can't decode {what}: {reason}")
}

/// Python's `PurePath.suffix`.
pub fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

/// `load_kicad_mod`: read and import one footprint file.
pub fn load_kicad_mod(path: &Path, display: &str, strict: bool) -> Result<FootprintImportResult> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    if suffix(&name).to_lowercase() != ".kicad_mod" {
        return import(format!("{display}: expected a .kicad_mod footprint file"));
    }
    let data = std::fs::read(path)
        .map_err(|failure| ImportError::Import(format!("cannot read {display}: {}", os_error(&failure, display))))?;
    let body = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&data);
    let text = std::str::from_utf8(body)
        .map_err(|failure| ImportError::Import(format!("cannot read {display}: {}", utf8_error(body, failure))))?;
    let mut result = parse_kicad_mod(text, display, strict)?;
    result.footprint.metadata.insert("source_sha256".to_owned(), hex(&data));
    Ok(result)
}

fn os_error(failure: &std::io::Error, filename: &str) -> String {
    let text = failure.to_string();
    let message = text.rfind(" (os error ").map_or(text.clone(), |index| text[..index].to_owned());
    match failure.raw_os_error() {
        Some(code) => format!("[Errno {code}] {message}: {}", repr(filename)),
        None => message,
    }
}

fn tag(node: &List) -> Option<&str> {
    match node.first() {
        Some(Node::Atom(text)) => Some(text),
        _ => None,
    }
}

fn lists(items: &[Node]) -> impl Iterator<Item = &List> {
    items.iter().filter_map(|item| match item {
        Node::List(list) => Some(list),
        Node::Atom(_) => None,
    })
}

fn children<'a>(node: &'a List, name: &str) -> Vec<&'a List> {
    lists(&node[1..]).filter(|child| tag(child) == Some(name)).collect()
}

fn first<'a>(node: &'a List, name: &str) -> Option<&'a List> {
    if node.is_empty() {
        return None;
    }
    lists(&node[1..]).find(|child| tag(child) == Some(name))
}

fn required_child<'a>(node: &'a List, name: &str, source: &str, description: &str) -> Result<&'a List> {
    first(node, name).ok_or_else(|| ImportError::Import(format!("{source}: missing {description}")))
}

fn atom(node: &List, index: usize) -> Option<&str> {
    match node.get(index) {
        Some(Node::Atom(text)) => Some(text),
        _ => None,
    }
}

fn required_atom<'a>(node: &'a List, index: usize, source: &str, description: &str) -> Result<&'a str> {
    atom(node, index).ok_or_else(|| ImportError::Import(format!("{source}: missing {description}")))
}

fn atoms(items: &[Node]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|item| match item {
            Node::Atom(text) => Some(text.as_str()),
            Node::List(_) => None,
        })
        .collect()
}

fn decimal(text: &str, source: &str, description: &str) -> Result<Decimal> {
    Decimal::parse(text)
        .map_err(|_| ImportError::Import(format!("{source}: invalid {description} value {}", repr(text))))
}

/// `nm_from_mm` of a parsed decimal: half-up integer nanometres.
fn number_nm(text: &str, source: &str, description: &str) -> Result<i64> {
    let number = decimal(text, source, description)?;
    let scaled = number.mul(&Decimal::from_i64(1_000_000)).map_err(|failure| value(failure.to_string()))?;
    scaled
        .to_integral_value(Rounding::HalfUp)
        .to_bigint()
        .to_i64()
        .ok_or_else(|| value(format!("{source}: {description} value {} is out of range", repr(text))))
}

fn point(node: &List, source: &str) -> Result<Point> {
    Ok(Point::new(
        number_nm(required_atom(node, 1, source, "x coordinate")?, source, "x")?,
        number_nm(required_atom(node, 2, source, "y coordinate")?, source, "y")?,
    ))
}

fn size(node: &List, source: &str) -> Result<Size> {
    let width = number_nm(required_atom(node, 1, source, "width")?, source, "width")?;
    let height = number_nm(required_atom(node, 2, source, "height")?, source, "height")?;
    Size::new(width, height).map_err(value)
}

struct Token {
    value: String,
    line: usize,
    column: usize,
}

fn tokenize(text: &str, source: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let (mut index, mut line, mut column) = (0, 1, 1);
    while index < chars.len() {
        let character = chars[index];
        if python_isspace(character) {
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
            index += 1;
            continue;
        }
        if character == '#' {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
                column += 1;
            }
            continue;
        }
        if character == '(' || character == ')' {
            tokens.push(Token { value: character.to_string(), line, column });
            index += 1;
            column += 1;
            continue;
        }
        let (token_line, token_column) = (line, column);
        if character == '"' {
            index += 1;
            column += 1;
            let mut value = String::new();
            while index < chars.len() && chars[index] != '"' {
                let mut current = chars[index];
                if current == '\\' {
                    index += 1;
                    column += 1;
                    if index >= chars.len() {
                        break;
                    }
                    current = match chars[index] {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    };
                }
                value.push(current);
                if current == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
                index += 1;
            }
            if index >= chars.len() {
                return import(format!("{source}:{token_line}:{token_column}: unterminated string"));
            }
            index += 1;
            column += 1;
            tokens.push(Token { value, line: token_line, column: token_column });
            continue;
        }
        let start = index;
        while index < chars.len() && !python_isspace(chars[index]) && chars[index] != '(' && chars[index] != ')' {
            index += 1;
            column += 1;
        }
        tokens.push(Token { value: chars[start..index].iter().collect(), line: token_line, column: token_column });
    }
    Ok(tokens)
}

fn parse_sexpr(text: &str, source: &str) -> Result<List> {
    let tokens = tokenize(text, source)?;
    if tokens.first().is_none_or(|token| token.value != "(") {
        return import(format!("{source}: expected '(' at start of footprint"));
    }
    fn parse_list(tokens: &[Token], index: &mut usize, source: &str) -> Result<List> {
        let mut result = Vec::new();
        while *index < tokens.len() {
            let token = &tokens[*index];
            *index += 1;
            if token.value == "(" {
                result.push(Node::List(parse_list(tokens, index, source)?));
            } else if token.value == ")" {
                return Ok(result);
            } else {
                result.push(Node::Atom(token.value.clone()));
            }
        }
        import(format!("{source}: unclosed '(' expression"))
    }
    let mut index = 1;
    let root = parse_list(&tokens, &mut index, source)?;
    if index != tokens.len() {
        let token = &tokens[index];
        return import(format!("{source}:{}:{}: unexpected token after footprint", token.line, token.column));
    }
    Ok(root)
}

fn layer_map(name: &str) -> Option<FootprintLayer> {
    Some(match name {
        "F.SilkS" => FootprintLayer::Silkscreen,
        "F.Fab" => FootprintLayer::Fabrication,
        "F.CrtYd" => FootprintLayer::Courtyard,
        "F.Adhes" => FootprintLayer::Adhesive,
        "Dwgs.User" | "User.Drawings" => FootprintLayer::Documentation,
        "F.Mask" => FootprintLayer::SolderMask,
        "F.Paste" => FootprintLayer::SolderPaste,
        _ => return None,
    })
}

/// Python's `repr` of a tuple of strings.
fn tuple_repr(items: &[&str]) -> String {
    match items {
        [single] => format!("({},)", repr(single)),
        _ => format!("({})", items.iter().map(|item| repr(item)).collect::<Vec<_>>().join(", ")),
    }
}

/// `parse_kicad_mod`: one footprint as backend-neutral physical IR.
pub fn parse_kicad_mod(text: &str, source: &str, strict: bool) -> Result<FootprintImportResult> {
    let root = parse_sexpr(text, source)?;
    if !matches!(tag(&root), Some("footprint" | "module")) {
        let found = tag(&root).map_or("None".to_owned(), repr);
        return import(format!("{source}: root expression must be 'footprint', got {found}"));
    }
    let name = required_atom(&root, 1, source, "footprint name")?.to_owned();
    if let Some(layer) = first(&root, "layer") {
        let layer = required_atom(layer, 1, source, "footprint layer")?;
        if layer != "F.Cu" {
            return import(format!("{source}: only front-side footprints are supported; got layer {}", repr(layer)));
        }
    }
    let version = first(&root, "version").and_then(|node| atom(node, 1)).map(str::to_owned);
    let generator = first(&root, "generator").and_then(|node| atom(node, 1)).unwrap_or_default().to_owned();
    let mut warnings: Vec<String> = Vec::new();
    let clearance_nm = match first(&root, "clearance") {
        Some(node) => {
            Some(number_nm(required_atom(node, 1, source, "footprint clearance")?, source, "footprint clearance")?)
        }
        None => None,
    };
    let mut pads = Vec::new();
    let mut graphics = Vec::new();
    let mut keepouts = Vec::new();
    let mut attributes: IndexSet<String> = IndexSet::new();
    let mut duplicate_jumpers = false;
    let mut internal_groups: Vec<InternalPadGroup> = Vec::new();
    for child in lists(root.get(2..).unwrap_or_default()) {
        let Some(child_tag) = tag(child) else { continue };
        match child_tag {
            "pad" => pads.push(parse_pad(child, source)?),
            "zone" => keepouts.push(parse_keepout(child, source, keepouts.len())?),
            "fp_line" | "fp_rect" | "fp_circle" | "fp_arc" | "fp_poly" => {
                if let Some(graphic) = parse_graphic(child, source, &mut warnings)? {
                    graphics.push(graphic);
                }
            }
            "fp_text" | "fp_text_box" => {
                let kind = atom(child, 1).filter(|kind| !kind.is_empty()).unwrap_or("user");
                if kind != "reference" && kind != "value" {
                    warnings.push(format!("ignored footprint text ({kind})"));
                }
            }
            "model" => warnings.push("ignored 3D model reference".to_owned()),
            "duplicate_pad_numbers_are_jumpers" => {
                let setting = required_atom(child, 1, source, "duplicate pad jumper setting")?;
                if setting != "yes" && setting != "no" {
                    return import(format!("{source}: invalid duplicate pad jumper setting"));
                }
                duplicate_jumpers = setting == "yes";
            }
            "jumper_pad_groups" => {
                for group in &child[1..] {
                    let Node::List(numbers) = group else {
                        return import(format!("{source}: jumper groups require lists of pad numbers"));
                    };
                    let numbers: Option<Vec<String>> = numbers
                        .iter()
                        .map(|number| match number {
                            Node::Atom(text) => Some(text.clone()),
                            Node::List(_) => None,
                        })
                        .collect();
                    let Some(numbers) = numbers else {
                        return import(format!("{source}: jumper groups require lists of pad numbers"));
                    };
                    internal_groups.push(
                        InternalPadGroup::new(numbers).map_err(|e| ImportError::Import(format!("{source}: {e}")))?,
                    );
                }
            }
            "embedded_fonts" => {
                if required_atom(child, 1, source, "embedded fonts setting")? != "no" {
                    warnings.push("ignored embedded footprint fonts".to_owned());
                }
            }
            "attr" => {
                let items: IndexSet<&str> = atoms(&child[1..]).into_iter().collect();
                let mut unsupported: Vec<&str> = items
                    .iter()
                    .copied()
                    .filter(|item| {
                        !["smd", "through_hole", "exclude_from_bom", "exclude_from_pos_files"].contains(item)
                    })
                    .collect();
                if !unsupported.is_empty() {
                    unsupported.sort_unstable();
                    let listed = unsupported.iter().map(|item| repr(item)).collect::<Vec<_>>().join(", ");
                    return import(format!("{source}: unsupported footprint attributes [{listed}]"));
                }
                attributes.extend(items.into_iter().map(str::to_owned));
            }
            "solder_mask_margin"
            | "solder_paste_margin"
            | "solder_paste_ratio"
            | "zone_connect"
            | "thermal_width"
            | "thermal_gap"
            | "private_layers"
            | "net_tie_pad_groups" => {
                return import(format!(
                    "{source}: unsupported fabrication-critical footprint setting {}",
                    repr(child_tag)
                ));
            }
            "version" | "generator" | "generator_version" | "layer" | "clearance" | "descr" | "tags" | "property"
            | "uuid" | "tedit" => {}
            other => {
                if !other.is_empty() {
                    warnings.push(format!("ignored unsupported footprint item {}", repr(other)));
                }
            }
        }
    }
    if pads.is_empty() {
        warnings.push("footprint contains no pads".to_owned());
    }
    let warnings: Vec<String> = warnings.into_iter().collect::<IndexSet<_>>().into_iter().collect();
    if strict && let Some(first) = warnings.first() {
        return import(format!("{source}: {first}"));
    }
    let mut metadata = IndexMap::new();
    metadata.insert("source_format".to_owned(), "kicad_mod".to_owned());
    metadata.insert("source_path".to_owned(), source.to_owned());
    metadata.insert("source_sha256".to_owned(), hex(text.as_bytes()));
    if let Some(version) = version.as_ref().filter(|version| !version.is_empty()) {
        metadata.insert("source_version".to_owned(), version.clone());
    }
    if !generator.is_empty() {
        metadata.insert("source_generator".to_owned(), generator);
    }
    if !warnings.is_empty() {
        metadata.insert("import_warnings".to_owned(), warnings.join(" | "));
    }
    if duplicate_jumpers {
        let mut counts: IndexMap<&str, usize> = IndexMap::new();
        for pad in pads.iter().filter(|pad| !pad.number.is_empty() && pad.kind.is_electrical()) {
            *counts.entry(&pad.number).or_default() += 1;
        }
        for (number, count) in counts {
            if count > 1 {
                internal_groups.push(InternalPadGroup::new(vec![number.to_owned()]).map_err(value)?);
            }
        }
    }
    let wrap = |message: String| ImportError::Import(format!("{source}: {message}"));
    let footprint = PhysicalFootprint {
        name: name.clone(),
        body_size: bounding_size(&pads, &graphics),
        courtyard: courtyard_polygon(&graphics),
        pads,
        source_library_id: Some(name),
        graphics,
        keepouts,
        metadata,
        height_nm: None,
        clearance_nm,
        exclude_from_bom: attributes.contains("exclude_from_bom"),
        exclude_from_pos_files: attributes.contains("exclude_from_pos_files"),
        internal_pad_groups: merge_internal_pad_groups(&internal_groups).map_err(wrap)?,
    }
    .validated()
    .map_err(wrap)?;
    Ok(FootprintImportResult { footprint, warnings, source_version: version })
}

fn parse_keepout(node: &List, source: &str, index: usize) -> Result<CopperKeepout> {
    let keepout = required_child(node, "keepout", source, "footprint keepout")?;
    let Some(layer_node) = first(node, "layer").or_else(|| first(node, "layers")) else {
        return import(format!("{source}: footprint keepout has no layers"));
    };
    let raw = atoms(&layer_node[1..]);
    if raw.is_empty() || raw.iter().any(|item| CopperLayer::parse(item).is_none() && *item != "F.CrtYd") {
        return import(format!("{source}: unsupported footprint keepout layers {}", tuple_repr(&raw)));
    }
    if !raw.iter().any(|item| CopperLayer::parse(item).is_some()) {
        return import(format!("{source}: footprint keepout has no copper layer"));
    }
    if let Some(placement) = first(node, "placement")
        && let Some(enabled) = first(placement, "enabled")
        && required_atom(enabled, 1, source, "placement keepout state")? != "no"
    {
        return import(format!("{source}: active footprint placement keepouts are unsupported"));
    }
    for name in ["net", "net_name"] {
        if let Some(item) = first(node, name)
            && !["", "0"].contains(&required_atom(item, 1, source, name)?)
        {
            return import(format!("{source}: connected footprint zones are unsupported"));
        }
    }
    let mut blocked: IndexMap<&str, bool> = IndexMap::new();
    for item in lists(&keepout[1..]) {
        let rule = tag(item).unwrap_or_default();
        if !["tracks", "vias", "pads", "copperpour", "footprints"].contains(&rule) {
            let shown = tag(item).map_or("None".to_owned(), repr);
            return import(format!("{source}: unsupported footprint keepout rule {shown}"));
        }
        let setting = required_atom(item, 1, source, &format!("keepout {rule}"))?;
        if setting != "allowed" && setting != "not_allowed" {
            return import(format!("{source}: invalid keepout {rule} rule"));
        }
        blocked.insert(rule, setting == "not_allowed");
    }
    if !["tracks", "vias", "pads", "copperpour"].iter().all(|rule| blocked.contains_key(rule)) {
        return import(format!("{source}: incomplete footprint keepout rules"));
    }
    let polygon = required_child(node, "polygon", source, "keepout polygon")?;
    let pts = required_child(polygon, "pts", source, "keepout polygon points")?;
    if lists(&pts[1..]).any(|item| tag(item) != Some("xy")) {
        return import(format!("{source}: unsupported keepout polygon item"));
    }
    let vertices = children(pts, "xy").into_iter().map(|item| point(item, source)).collect::<Result<Vec<_>>>()?;
    let vertices =
        polygon_ring(&vertices).map_err(|e| ImportError::Import(format!("{source}: invalid keepout polygon: {e}")))?;
    let allowed = [
        "layer",
        "layers",
        "uuid",
        "name",
        "hatch",
        "connect_pads",
        "min_thickness",
        "keepout",
        "placement",
        "fill",
        "polygon",
        "net",
        "net_name",
    ];
    for child in lists(&node[1..]) {
        if !tag(child).is_some_and(|name| allowed.contains(&name)) {
            let shown = tag(child).map_or("None".to_owned(), repr);
            return import(format!("{source}: unsupported footprint keepout item {shown}"));
        }
    }
    let name = first(node, "name").and_then(|node| atom(node, 1)).filter(|name| !name.is_empty()).unwrap_or("keepout");
    let layers: IndexSet<CopperLayer> = raw.iter().filter_map(|item| CopperLayer::parse(item)).collect();
    let keepout = CopperKeepout {
        id: format!("{name}-{index}"),
        layers: layers.into_iter().collect(),
        vertices,
        block_tracks: blocked["tracks"],
        block_vias: blocked["vias"],
        block_pads: blocked["pads"],
        block_zones: blocked["copperpour"],
        block_footprints: blocked.get("footprints").copied().unwrap_or(false),
    };
    keepout.validate().map_err(value)?;
    Ok(keepout)
}

fn validate_pad_layers(kind: PadKind, layers: &[&str], source: &str) -> Result<(bool, bool)> {
    let set: IndexSet<&str> = layers.iter().copied().collect();
    if matches!(kind, PadKind::Smd | PadKind::Aperture) {
        let allowed = ["F.Cu", "F.Mask", "F.Paste"];
        if set.contains("F.Cu") != (kind == PadKind::Smd) || set.iter().any(|layer| !allowed.contains(layer)) {
            return import(format!("{source}: unsupported {} pad layers {}", kind.value(), tuple_repr(layers)));
        }
        return Ok((set.contains("F.Mask"), set.contains("F.Paste")));
    }
    if !set.contains("*.Cu") || set.iter().any(|layer| !["*.Cu", "*.Mask"].contains(layer)) {
        return import(format!("{source}: unsupported through-hole pad layers {}", tuple_repr(layers)));
    }
    Ok((set.contains("*.Mask"), false))
}

fn parse_drill(node: Option<&List>, kind: PadKind, source: &str) -> Result<Option<Size>> {
    let drilled = !matches!(kind, PadKind::Smd | PadKind::Aperture);
    let Some(node) = node else {
        if !drilled {
            return Ok(None);
        }
        return import(format!("{source}: through-hole pad is missing a drill"));
    };
    if !drilled {
        return import(format!("{source}: non-drilled pad unexpectedly contains a drill"));
    }
    if node[1..].iter().any(|item| matches!(item, Node::List(_))) {
        return import(format!("{source}: offset or compound drill definitions are not supported"));
    }
    let values = atoms(&node[1..]);
    let Some(first_value) = values.first() else { return import(format!("{source}: empty drill definition")) };
    if *first_value == "oval" {
        if values.len() != 3 {
            return import(format!("{source}: oval drill requires width and height"));
        }
        let width = number_nm(values[1], source, "drill width")?;
        let height = number_nm(values[2], source, "drill height")?;
        return Ok(Some(Size::new(width, height).map_err(value)?));
    }
    if values.len() != 1 {
        let listed = values.iter().map(|item| repr(item)).collect::<Vec<_>>().join(", ");
        return import(format!("{source}: unsupported drill definition [{listed}]"));
    }
    let diameter = number_nm(first_value, source, "drill diameter")?;
    Ok(Some(Size::new(diameter, diameter).map_err(value)?))
}

fn parse_pad(node: &List, source: &str) -> Result<FootprintPad> {
    let number = required_atom(node, 1, source, "pad number")?.to_owned();
    let kind_text = required_atom(node, 2, source, "pad kind")?;
    let shape_text = required_atom(node, 3, source, "pad shape")?;
    let mut kind = match kind_text {
        "smd" | "connect" => PadKind::Smd,
        "thru_hole" => PadKind::ThroughHole,
        "np_thru_hole" => PadKind::NonPlatedThroughHole,
        other => return import(format!("{source}: unsupported pad kind {}", repr(other))),
    };
    let shape = match shape_text {
        "circle" => PadShape::Circle,
        "oval" => PadShape::Oval,
        "rect" => PadShape::Rectangle,
        "roundrect" => PadShape::Roundrect,
        other => return import(format!("{source}: unsupported fabrication-critical pad shape {}", repr(other))),
    };
    let at = required_child(node, "at", source, "pad position")?;
    let position = point(at, source)?;
    let rotation = decimal(atom(at, 3).filter(|text| !text.is_empty()).unwrap_or("0"), source, "pad rotation")?;
    let size = size(required_child(node, "size", source, "pad size")?, source)?;
    let layers_node = required_child(node, "layers", source, "pad layers")?;
    let layers = atoms(&layers_node[1..]);
    if kind == PadKind::Smd
        && !layers.contains(&"F.Cu")
        && layers.iter().all(|layer| ["F.Mask", "F.Paste"].contains(layer))
    {
        kind = PadKind::Aperture;
    }
    let (has_mask, has_paste) = validate_pad_layers(kind, &layers, source)?;
    if kind_text == "connect" && (!has_mask || has_paste) {
        return import(format!("{source}: connector-contact pad {} must have mask but no paste", repr(&number)));
    }
    let drill = parse_drill(first(node, "drill"), kind, source)?;
    let mut ratio_ppm = 250_000;
    if let Some(ratio) = first(node, "roundrect_rratio") {
        let ratio = decimal(required_atom(ratio, 1, source, "roundrect ratio")?, source, "roundrect ratio")?;
        let scaled = ratio.mul(&Decimal::from_i64(1_000_000)).map_err(|failure| value(failure.to_string()))?;
        ratio_ppm = scaled.to_integral_value(Rounding::HalfUp).to_bigint().to_i64().unwrap_or(i64::MAX);
    }
    let mut heatsink = false;
    if let Some(property) = first(node, "property") {
        let name = required_atom(property, 1, source, "pad property")?;
        if name != "pad_prop_heatsink" {
            return import(format!("{source}: unsupported fabrication-critical pad property {}", repr(name)));
        }
        heatsink = true;
    }
    let zone_connection = match first(node, "zone_connect") {
        None => None,
        Some(zone) => Some(match required_atom(zone, 1, source, "pad zone connection")? {
            "1" => ZoneConnection::None,
            "2" => ZoneConnection::Thermal,
            "3" => ZoneConnection::Solid,
            "4" => ZoneConnection::ThtThermal,
            other => return import(format!("{source}: unsupported pad zone connection {}", repr(other))),
        }),
    };
    let mut remove_unused_layers = false;
    if let Some(remove) = first(node, "remove_unused_layers") {
        let setting = required_atom(remove, 1, source, "remove unused layers")?;
        if setting != "yes" && setting != "no" {
            return import(format!("{source}: invalid remove_unused_layers value"));
        }
        remove_unused_layers = setting == "yes";
    }
    if let Some(angle) = first(node, "thermal_bridge_angle")
        && required_atom(angle, 1, source, "pad thermal angle")? != "45"
    {
        return import(format!("{source}: pad {} uses unsupported non-default thermal bridge angle", repr(&number)));
    }
    let allowed = [
        "at",
        "size",
        "layers",
        "roundrect_rratio",
        "drill",
        "uuid",
        "tstamp",
        "pinfunction",
        "pintype",
        "property",
        "zone_connect",
        "remove_unused_layers",
        "thermal_bridge_angle",
    ];
    for child in lists(node.get(4..).unwrap_or_default()) {
        if !tag(child).is_some_and(|name| allowed.contains(&name)) {
            let shown = tag(child).map_or("None".to_owned(), repr);
            return import(format!("{source}: pad {} uses unsupported fabrication modifier {shown}", repr(&number)));
        }
    }
    FootprintPad {
        number: number.clone(),
        position,
        size,
        kind,
        shape,
        rotation_degrees: rotation,
        drill,
        roundrect_ratio_ppm: ratio_ppm,
        has_solder_mask: has_mask,
        has_solder_paste: has_paste,
        zone_connection,
        heatsink,
        remove_unused_layers,
        connector_contact: kind_text == "connect",
    }
    .validated()
    .map_err(|e| ImportError::Import(format!("{source}: pad {}: {e}", repr(&number))))
}

fn graphic_width(node: &List, source: &str) -> Result<i64> {
    let width = match first(node, "stroke") {
        Some(stroke) => first(stroke, "width"),
        None => first(node, "width"),
    };
    let Some(width) = width else { return import(format!("{source}: footprint graphic is missing stroke width")) };
    number_nm(required_atom(width, 1, source, "stroke width")?, source, "stroke width")
}

fn graphic_filled(node: &List, source: &str) -> Result<bool> {
    let Some(fill) = first(node, "fill") else { return Ok(false) };
    let setting = required_atom(fill, 1, source, "graphic fill")?;
    if !["none", "solid", "no", "yes"].contains(&setting) {
        return import(format!("{source}: unsupported graphic fill {}", repr(setting)));
    }
    Ok(setting == "solid" || setting == "yes")
}

fn parse_graphic(node: &List, source: &str, warnings: &mut Vec<String>) -> Result<Option<FootprintGraphic>> {
    let layer_node = required_child(node, "layer", source, "graphic layer")?;
    let layer_name = required_atom(layer_node, 1, source, "graphic layer")?;
    let Some(layer) = layer_map(layer_name) else {
        if layer_name == "F.Cu" || layer_name == "Edge.Cuts" {
            return import(format!("{source}: unsupported fabrication-critical graphic layer {}", repr(layer_name)));
        }
        warnings.push(format!("ignored graphic on unsupported layer {}", repr(layer_name)));
        return Ok(None);
    };
    let width_nm = graphic_width(node, source)?;
    let filled = graphic_filled(node, source)?;
    let child = |name: &str, description: &str| -> Result<Point> {
        point(required_child(node, name, source, description)?, source)
    };
    let graphic = match tag(node).unwrap_or_default() {
        "fp_line" => {
            let (start, end) = (child("start", "line start")?, child("end", "line end")?);
            if start == end {
                warnings.push(format!("ignored zero-length graphic on {layer_name}"));
                return Ok(None);
            }
            FootprintGraphic::Line { start, end, width_nm, layer }
        }
        "fp_rect" => FootprintGraphic::Rectangle {
            start: child("start", "rectangle start")?,
            end: child("end", "rectangle end")?,
            width_nm,
            layer,
            filled,
        },
        "fp_circle" => FootprintGraphic::Circle {
            center: child("center", "circle center")?,
            end: child("end", "circle end")?,
            width_nm,
            layer,
            filled,
        },
        "fp_arc" => {
            if filled {
                return import(format!("{source}: filled arcs are not supported"));
            }
            let Some(mid) = first(node, "mid") else {
                return import(format!("{source}: legacy angle-based arcs are not supported yet"));
            };
            let start = child("start", "arc start")?;
            let midpoint = point(mid, source)?;
            let end = child("end", "arc end")?;
            FootprintGraphic::Arc { start, midpoint, end, width_nm, layer }
        }
        _ => {
            let pts = required_child(node, "pts", source, "polygon points")?;
            let points = children(pts, "xy").into_iter().map(|item| point(item, source)).collect::<Result<Vec<_>>>()?;
            FootprintGraphic::Polygon { points, width_nm, layer, filled }
        }
    };
    graphic.validated().map(Some).map_err(value)
}

fn ceil_sqrt(squared: i128) -> i64 {
    let root = BigInt::from(squared).sqrt();
    let root = root.to_i64().expect("bounded radius");
    if i128::from(root) * i128::from(root) == squared { root } else { root + 1 }
}

fn bounding_size(pads: &[FootprintPad], graphics: &[FootprintGraphic]) -> Size {
    let mut points: Vec<Point> = Vec::new();
    for pad in pads {
        let (half_width, half_height) = (pad.size.width_nm / 2, pad.size.height_nm / 2);
        points.push(Point::new(pad.position.x_nm - half_width, pad.position.y_nm - half_height));
        points.push(Point::new(pad.position.x_nm + half_width, pad.position.y_nm + half_height));
    }
    for graphic in graphics {
        match graphic {
            FootprintGraphic::Line { start, end, .. } | FootprintGraphic::Rectangle { start, end, .. } => {
                points.extend([*start, *end]);
            }
            FootprintGraphic::Arc { start, midpoint, end, .. } => points.extend([*start, *end, *midpoint]),
            FootprintGraphic::Circle { center, end, .. } => {
                let (dx, dy) = (i128::from(end.x_nm - center.x_nm), i128::from(end.y_nm - center.y_nm));
                let radius = ceil_sqrt(dx * dx + dy * dy);
                points.push(Point::new(center.x_nm - radius, center.y_nm - radius));
                points.push(Point::new(center.x_nm + radius, center.y_nm + radius));
            }
            FootprintGraphic::Polygon { points: polygon, .. } => points.extend(polygon.iter().copied()),
        }
    }
    if points.is_empty() {
        return Size { width_nm: 1_000_000, height_nm: 1_000_000 };
    }
    let width = points.iter().map(|p| p.x_nm).max().unwrap_or(0) - points.iter().map(|p| p.x_nm).min().unwrap_or(0);
    let height = points.iter().map(|p| p.y_nm).max().unwrap_or(0) - points.iter().map(|p| p.y_nm).min().unwrap_or(0);
    Size { width_nm: width.max(1), height_nm: height.max(1) }
}

fn courtyard_polygon(graphics: &[FootprintGraphic]) -> Vec<Point> {
    let courtyard: Vec<&FootprintGraphic> =
        graphics.iter().filter(|graphic| graphic.layer() == FootprintLayer::Courtyard).collect();
    let polygons: Vec<&Vec<Point>> = courtyard
        .iter()
        .filter_map(|graphic| match graphic {
            FootprintGraphic::Polygon { points, .. } => Some(points),
            _ => None,
        })
        .collect();
    if polygons.len() == 1 {
        return polygons[0].clone();
    }
    let rectangles: Vec<(Point, Point)> = courtyard
        .iter()
        .filter_map(|graphic| match graphic {
            FootprintGraphic::Rectangle { start, end, .. } => Some((*start, *end)),
            _ => None,
        })
        .collect();
    if rectangles.len() == 1 && courtyard.len() == 1 {
        let (start, end) = rectangles[0];
        return vec![start, Point::new(end.x_nm, start.y_nm), end, Point::new(start.x_nm, end.y_nm)];
    }
    let mut points: Vec<Point> = Vec::new();
    for graphic in &courtyard {
        match graphic {
            FootprintGraphic::Line { start, end, .. } | FootprintGraphic::Rectangle { start, end, .. } => {
                points.extend([*start, *end]);
            }
            FootprintGraphic::Arc { start, midpoint, end, .. } => points.extend([*start, *end, *midpoint]),
            FootprintGraphic::Circle { center, end, .. } => {
                let radius = round_int(((end.x_nm - center.x_nm) as f64).hypot((end.y_nm - center.y_nm) as f64));
                points.push(Point::new(center.x_nm - radius, center.y_nm - radius));
                points.push(Point::new(center.x_nm + radius, center.y_nm + radius));
            }
            FootprintGraphic::Polygon { points: polygon, .. } => points.extend(polygon.iter().copied()),
        }
    }
    if points.is_empty() {
        return Vec::new();
    }
    let min_x = points.iter().map(|p| p.x_nm).min().unwrap_or(0);
    let min_y = points.iter().map(|p| p.y_nm).min().unwrap_or(0);
    let max_x = points.iter().map(|p| p.x_nm).max().unwrap_or(0);
    let max_y = points.iter().map(|p| p.y_nm).max().unwrap_or(0);
    vec![Point::new(min_x, min_y), Point::new(max_x, min_y), Point::new(max_x, max_y), Point::new(min_x, max_y)]
}

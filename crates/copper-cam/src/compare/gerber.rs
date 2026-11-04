//! A Gerber reader for comparison: the RS-274X subset KiCad and this crate
//! write (absolute leading-zero-omitted coordinates, standard and macro
//! apertures, round-pen strokes, multi-quadrant arcs, regions, polarity).
//! Every image object becomes disks, polygons and capsules in Gerber
//! nanometres. Unsupported commands are errors, not skipped.

use indexmap::IndexMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Disk { center: (f64, f64), radius: f64, dark: bool },
    Polygon { points: Vec<(f64, f64)>, dark: bool },
    Capsule { a: (f64, f64), b: (f64, f64), width: f64, dark: bool },
}

#[derive(Clone, Debug, Default)]
pub struct Image {
    pub file_function: Option<String>,
    pub negative: bool,
    pub ops: Vec<Op>,
}

impl Image {
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        let mut grow = |x: f64, y: f64, r: f64| {
            let b = bounds.get_or_insert((x - r, y - r, x + r, y + r));
            *b = (b.0.min(x - r), b.1.min(y - r), b.2.max(x + r), b.3.max(y + r));
        };
        for op in &self.ops {
            match op {
                Op::Disk { center, radius, .. } => grow(center.0, center.1, *radius),
                Op::Polygon { points, .. } => points.iter().for_each(|p| grow(p.0, p.1, 0.0)),
                Op::Capsule { a, b, width, .. } => {
                    grow(a.0, a.1, width / 2.0);
                    grow(b.0, b.1, width / 2.0);
                }
            }
        }
        bounds
    }
}

/// An aperture as primitive shapes around its flash point (all dark).
type Aperture = Vec<Op>;

const ARC_TOLERANCE: f64 = 250.0;

fn rotate(p: (f64, f64), degrees: f64) -> (f64, f64) {
    if degrees == 0.0 {
        return p;
    }
    let (s, c) = degrees.to_radians().sin_cos();
    (p.0 * c - p.1 * s, p.0 * s + p.1 * c)
}

fn rectangle(cx: f64, cy: f64, w: f64, h: f64) -> Vec<(f64, f64)> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    vec![(cx - hw, cy - hh), (cx + hw, cy - hh), (cx + hw, cy + hh), (cx - hw, cy + hh)]
}

fn regular_polygon(cx: f64, cy: f64, diameter: f64, vertices: usize, rotation: f64) -> Vec<(f64, f64)> {
    (0..vertices)
        .map(|index| {
            let angle = (rotation + 360.0 * index as f64 / vertices as f64).to_radians();
            (cx + diameter / 2.0 * angle.cos(), cy + diameter / 2.0 * angle.sin())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// aperture macro expressions

struct Expression<'a> {
    text: &'a [u8],
    index: usize,
    variables: &'a IndexMap<usize, f64>,
}

impl Expression<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.index).copied()
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut value = self.product()?;
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.index += 1;
            let rhs = self.product()?;
            value = if op == b'+' { value + rhs } else { value - rhs };
        }
        Ok(value)
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut value = self.unary()?;
        while let Some(op @ (b'x' | b'X' | b'/')) = self.peek() {
            self.index += 1;
            let rhs = self.unary()?;
            value = if op == b'/' { value / rhs } else { value * rhs };
        }
        Ok(value)
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(b'-') => {
                self.index += 1;
                Ok(-self.unary()?)
            }
            Some(b'+') => {
                self.index += 1;
                self.unary()
            }
            Some(b'(') => {
                self.index += 1;
                let value = self.sum()?;
                if self.peek() != Some(b')') {
                    return Err("unbalanced parenthesis in aperture macro".to_owned());
                }
                self.index += 1;
                Ok(value)
            }
            Some(b'$') => {
                self.index += 1;
                let start = self.index;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.index += 1;
                }
                let number: usize = std::str::from_utf8(&self.text[start..self.index])
                    .ok()
                    .and_then(|digits| digits.parse().ok())
                    .ok_or("bad macro variable")?;
                Ok(self.variables.get(&number).copied().unwrap_or(0.0))
            }
            _ => {
                let start = self.index;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                    self.index += 1;
                }
                std::str::from_utf8(&self.text[start..self.index])
                    .ok()
                    .and_then(|number| number.parse().ok())
                    .ok_or_else(|| format!("bad macro expression {:?}", String::from_utf8_lossy(self.text)))
            }
        }
    }
}

fn evaluate(text: &str, variables: &IndexMap<usize, f64>) -> Result<f64, String> {
    let compact: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut expression = Expression { text: &compact, index: 0, variables };
    let value = expression.sum()?;
    if expression.index != compact.len() {
        return Err(format!("bad macro expression {text:?}"));
    }
    Ok(value)
}

/// Instantiate macro `body` with `parameters` (in file units, scaled by `unit`).
fn instantiate(body: &[String], parameters: &[f64], unit: f64) -> Result<Aperture, String> {
    let mut variables: IndexMap<usize, f64> = parameters.iter().enumerate().map(|(i, &v)| (i + 1, v)).collect();
    let mut shapes = Vec::new();
    for statement in body {
        let statement = statement.trim();
        // Primitive 0 is a comment (its spaces are already stripped).
        if statement.is_empty() || statement.starts_with('0') {
            continue;
        }
        if let Some(assignment) = statement.strip_prefix('$') {
            let (name, value) = assignment.split_once('=').ok_or("bad macro assignment")?;
            let index: usize = name.trim().parse().map_err(|_| "bad macro variable")?;
            let value = evaluate(value, &variables)?;
            variables.insert(index, value);
            continue;
        }
        let fields: Vec<f64> =
            statement.split(',').map(|field| evaluate(field, &variables)).collect::<Result<_, _>>()?;
        let code = fields[0] as i64;
        let at =
            |index: usize| fields.get(index).copied().ok_or_else(|| format!("short macro primitive {statement:?}"));
        if at(1)? == 0.0 {
            return Err("clear-exposure macro primitives are not compared".to_owned());
        }
        match code {
            1 => {
                let rotation = fields.get(5).copied().unwrap_or(0.0);
                let center = rotate((at(3)? * unit, at(4)? * unit), rotation);
                shapes.push(Op::Disk { center, radius: at(2)? * unit / 2.0, dark: true });
            }
            20 => {
                let (w, a, b) = (at(2)? * unit, (at(3)? * unit, at(4)? * unit), (at(5)? * unit, at(6)? * unit));
                let rotation = at(7)?;
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let length = dx.hypot(dy);
                if length > 0.0 {
                    let (nx, ny) = (-dy / length * w / 2.0, dx / length * w / 2.0);
                    let points =
                        [(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)];
                    shapes.push(Op::Polygon {
                        points: points.iter().map(|&p| rotate(p, rotation)).collect(),
                        dark: true,
                    });
                }
            }
            21 => {
                let points = rectangle(at(4)? * unit, at(5)? * unit, at(2)? * unit, at(3)? * unit);
                let rotation = at(6)?;
                shapes.push(Op::Polygon {
                    points: points.into_iter().map(|p| rotate(p, rotation)).collect(),
                    dark: true,
                });
            }
            4 => {
                let count = at(2)? as usize;
                let rotation = at(5 + 2 * count)?;
                let points: Vec<(f64, f64)> = (0..count)
                    .map(|i| Ok(rotate((at(3 + 2 * i)? * unit, at(4 + 2 * i)? * unit), rotation)))
                    .collect::<Result<_, String>>()?;
                shapes.push(Op::Polygon { points, dark: true });
            }
            5 => {
                let rotation = fields.get(6).copied().unwrap_or(0.0);
                let center = rotate((at(3)? * unit, at(4)? * unit), rotation);
                let points = regular_polygon(center.0, center.1, at(5)? * unit, at(2)? as usize, rotation);
                shapes.push(Op::Polygon { points, dark: true });
            }
            other => return Err(format!("aperture macro primitive {other} is not compared")),
        }
    }
    Ok(shapes)
}

fn standard_aperture(name: &str, parameters: &[f64], unit: f64) -> Result<Aperture, String> {
    let p =
        |index: usize| parameters.get(index).map(|value| value * unit).ok_or_else(|| format!("short {name} aperture"));
    if (name == "C" && parameters.len() > 1) || (matches!(name, "R" | "O") && parameters.len() > 2) {
        return Err("apertures with holes are not compared".to_owned());
    }
    Ok(match name {
        "C" => vec![Op::Disk { center: (0.0, 0.0), radius: p(0)? / 2.0, dark: true }],
        "R" => vec![Op::Polygon { points: rectangle(0.0, 0.0, p(0)?, p(1)?), dark: true }],
        "O" => {
            let (w, h) = (p(0)?, p(1)?);
            if w > h {
                vec![Op::Capsule { a: (-(w - h) / 2.0, 0.0), b: ((w - h) / 2.0, 0.0), width: h, dark: true }]
            } else {
                vec![Op::Capsule { a: (0.0, -(h - w) / 2.0), b: (0.0, (h - w) / 2.0), width: w, dark: true }]
            }
        }
        "P" => {
            let rotation = parameters.get(2).copied().unwrap_or(0.0);
            vec![Op::Polygon { points: regular_polygon(0.0, 0.0, p(0)?, parameters[1] as usize, rotation), dark: true }]
        }
        other => return Err(format!("unknown aperture template {other:?}")),
    })
}

fn translated(shape: &Op, dx: f64, dy: f64, dark: bool) -> Op {
    match shape {
        Op::Disk { center, radius, .. } => Op::Disk { center: (center.0 + dx, center.1 + dy), radius: *radius, dark },
        Op::Polygon { points, .. } => {
            Op::Polygon { points: points.iter().map(|p| (p.0 + dx, p.1 + dy)).collect(), dark }
        }
        Op::Capsule { a, b, width, .. } => {
            Op::Capsule { a: (a.0 + dx, a.1 + dy), b: (b.0 + dx, b.1 + dy), width: *width, dark }
        }
    }
}

fn arc_points(start: (f64, f64), end: (f64, f64), center: (f64, f64), clockwise: bool) -> Vec<(f64, f64)> {
    let radius = (start.0 - center.0).hypot(start.1 - center.1);
    let a0 = (start.1 - center.1).atan2(start.0 - center.0);
    let a1 = (end.1 - center.1).atan2(end.0 - center.0);
    let tau = std::f64::consts::TAU;
    let mut sweep = a1 - a0;
    if clockwise {
        while sweep >= 0.0 {
            sweep -= tau;
        }
    } else {
        while sweep <= 0.0 {
            sweep += tau;
        }
    }
    if (start.0 - end.0).abs() < 0.5 && (start.1 - end.1).abs() < 0.5 {
        sweep = if clockwise { -tau } else { tau };
    }
    let step =
        if radius > ARC_TOLERANCE { 2.0 * (1.0 - ARC_TOLERANCE / radius).acos() } else { std::f64::consts::FRAC_PI_4 };
    let count = ((sweep.abs() / step).ceil() as usize).clamp(1, 8192);
    let mut points: Vec<(f64, f64)> = (1..count)
        .map(|i| a0 + sweep * i as f64 / count as f64)
        .map(|a| (center.0 + radius * a.cos(), center.1 + radius * a.sin()))
        .collect();
    points.push(end);
    points
}

struct Reader {
    image: Image,
    unit_per_coordinate: f64,
    unit: f64,
    apertures: IndexMap<u32, Aperture>,
    macros: IndexMap<String, Vec<String>>,
    current: Option<u32>,
    position: (f64, f64),
    mode: u8,
    dark: bool,
    region: Option<Vec<Vec<(f64, f64)>>>,
    format_set: bool,
    decimals: i32,
}

impl Reader {
    fn coordinate(&self, digits: &str) -> Result<f64, String> {
        let value: i64 = digits.parse().map_err(|_| format!("bad coordinate {digits:?}"))?;
        Ok(value as f64 * self.unit_per_coordinate)
    }

    fn extended(&mut self, statements: &[String]) -> Result<(), String> {
        let Some(first) = statements.first() else { return Ok(()) };
        if let Some(name) = first.strip_prefix("AM") {
            self.macros.insert(name.to_owned(), statements[1..].to_vec());
            return Ok(());
        }
        for statement in statements {
            let statement = statement.as_str();
            if let Some(format) = statement.strip_prefix("FS") {
                let decimals = format
                    .strip_prefix("LA")
                    .and_then(|rest| rest.split_once('Y'))
                    .and_then(|(x, _)| x.strip_prefix('X'))
                    .and_then(|x| x.get(1..2))
                    .and_then(|digit| digit.parse::<i32>().ok())
                    .ok_or_else(|| format!("unsupported coordinate format {statement:?}"))?;
                self.unit_per_coordinate = self.unit / 10f64.powi(decimals);
                self.format_set = true;
                self.decimals = decimals;
            } else if let Some(units) = statement.strip_prefix("MO") {
                self.unit = match units {
                    "MM" => 1e6,
                    "IN" => 25.4e6,
                    other => return Err(format!("unknown unit {other:?}")),
                };
                self.unit_per_coordinate = self.unit / 10f64.powi(self.decimals);
            } else if let Some(definition) = statement.strip_prefix("ADD") {
                let digits: String = definition.chars().take_while(char::is_ascii_digit).collect();
                let code: u32 = digits.parse().map_err(|_| format!("bad aperture {statement:?}"))?;
                let rest = &definition[digits.len()..];
                let (name, parameters) = rest.split_once(',').unwrap_or((rest, ""));
                let parameters: Vec<f64> = if parameters.is_empty() {
                    Vec::new()
                } else {
                    parameters
                        .split('X')
                        .map(|p| p.parse::<f64>().map_err(|_| format!("bad aperture parameter {p:?}")))
                        .collect::<Result<_, _>>()?
                };
                let shapes = match self.macros.get(name) {
                    Some(body) => instantiate(body, &parameters, self.unit)?,
                    None => standard_aperture(name, &parameters, self.unit)?,
                };
                self.apertures.insert(code, shapes);
            } else if let Some(polarity) = statement.strip_prefix("LP") {
                self.dark = match polarity {
                    "D" => true,
                    "C" => false,
                    other => return Err(format!("unknown polarity {other:?}")),
                };
            } else if let Some(attribute) = statement.strip_prefix("TF.") {
                if let Some(function) = attribute.strip_prefix("FileFunction,") {
                    self.image.file_function = Some(function.to_owned());
                } else if let Some(polarity) = attribute.strip_prefix("FilePolarity,") {
                    self.image.negative = polarity == "Negative";
                }
            } else if statement.is_empty()
                || ["TA", "TO", "TD"].iter().any(|prefix| statement.starts_with(prefix))
                || matches!(statement, "LMN" | "LR0" | "LS1" | "IPPOS" | "SRX1Y1I0J0" | "SR")
            {
                // Attributes and identity transforms do not change the image.
            } else {
                return Err(format!("unsupported Gerber command %{statement}*%"));
            }
        }
        Ok(())
    }

    fn word(&mut self, mut word: &str) -> Result<bool, String> {
        while let Some(rest) = word.strip_prefix('G') {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let code: u32 = digits.parse().map_err(|_| format!("bad G code {word:?}"))?;
            word = &rest[digits.len()..];
            match code {
                4 => return Ok(true),
                1 => self.mode = 1,
                2 => self.mode = 2,
                3 => self.mode = 3,
                75 => {}
                74 => return Err("single-quadrant arcs are not compared".to_owned()),
                36 => self.region = Some(Vec::new()),
                37 => {
                    let contours = self.region.take().unwrap_or_default();
                    for points in contours.into_iter().filter(|points| points.len() > 2) {
                        self.image.ops.push(Op::Polygon { points, dark: self.dark });
                    }
                }
                54 | 55 | 70 | 71 | 90 => {}
                other => return Err(format!("unsupported G{other:02}")),
            }
        }
        if word.is_empty() {
            return Ok(true);
        }
        if matches!(word, "M02" | "M00" | "M30") {
            return Ok(false);
        }
        if let Some(code) = word.strip_prefix('D')
            && let Ok(code) = code.parse::<u32>()
            && code >= 10
        {
            if !self.apertures.contains_key(&code) {
                return Err(format!("undefined aperture D{code}"));
            }
            self.current = Some(code);
            return Ok(true);
        }
        // Coordinates and an operation code.
        let (mut x, mut y, mut i, mut j) = (None, None, 0.0, 0.0);
        let mut operation = None;
        let bytes = word.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let letter = bytes[index];
            index += 1;
            let start = index;
            while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'-' || bytes[index] == b'+')
            {
                index += 1;
            }
            let digits = &word[start..index];
            match letter {
                b'X' => x = Some(self.coordinate(digits)?),
                b'Y' => y = Some(self.coordinate(digits)?),
                b'I' => i = self.coordinate(digits)?,
                b'J' => j = self.coordinate(digits)?,
                b'D' => operation = Some(digits.parse::<u32>().map_err(|_| format!("bad D code {word:?}"))?),
                _ => return Err(format!("unsupported Gerber word {word:?}")),
            }
        }
        if !self.format_set {
            return Err("coordinates before %FS".to_owned());
        }
        let target = (x.unwrap_or(self.position.0), y.unwrap_or(self.position.1));
        let start = self.position;
        match operation {
            Some(2) => {
                if let Some(region) = &mut self.region {
                    region.push(vec![target]);
                }
            }
            Some(1) => {
                let path: Vec<(f64, f64)> = if self.mode == 1 {
                    vec![target]
                } else {
                    arc_points(start, target, (start.0 + i, start.1 + j), self.mode == 2)
                };
                if let Some(region) = &mut self.region {
                    if region.is_empty() {
                        region.push(vec![start]);
                    }
                    region.last_mut().expect("a contour").extend(path);
                } else {
                    let code = self.current.ok_or("draw without an aperture")?;
                    let width = match self.apertures[&code].as_slice() {
                        [Op::Disk { center: (0.0, 0.0), radius, .. }] => radius * 2.0,
                        [] => 0.0,
                        _ => return Err(format!("strokes with non-circular aperture D{code} are not compared")),
                    };
                    let mut from = start;
                    for point in path {
                        self.image.ops.push(Op::Capsule { a: from, b: point, width, dark: self.dark });
                        from = point;
                    }
                }
            }
            Some(3) => {
                let code = self.current.ok_or("flash without an aperture")?;
                let dark = self.dark;
                let shapes: Vec<Op> =
                    self.apertures[&code].iter().map(|shape| translated(shape, target.0, target.1, dark)).collect();
                self.image.ops.extend(shapes);
            }
            None => {}
            Some(other) => return Err(format!("unsupported D{other:02}")),
        }
        self.position = target;
        Ok(true)
    }
}

/// Parse a Gerber file into image operations.
pub fn read(text: &str) -> Result<Image, String> {
    let mut reader = Reader {
        image: Image::default(),
        unit_per_coordinate: 1.0,
        unit: 1e6,
        apertures: IndexMap::new(),
        macros: IndexMap::new(),
        current: None,
        position: (0.0, 0.0),
        mode: 1,
        dark: true,
        region: None,
        format_set: false,
        decimals: 6,
    };
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(block) = rest.strip_prefix('%') {
            let end = block.find('%').ok_or("unterminated extended command")?;
            let statements: Vec<String> = block[..end]
                .split('*')
                .map(|s| s.split_whitespace().collect::<String>())
                .filter(|s| !s.is_empty())
                .collect();
            reader.extended(&statements)?;
            rest = &block[end + 1..];
        } else {
            let end = rest.find('*').ok_or("unterminated Gerber word")?;
            let word: String = rest[..end].split_whitespace().collect();
            let comment = word.starts_with("G04");
            rest = &rest[end + 1..];
            if comment {
                continue;
            }
            if !reader.word(&word)? {
                break;
            }
        }
    }
    Ok(reader.image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kicad_round_rect_macro_flashes_as_its_primitives() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%AMRoundRect*\n0 comment*\n4,1,4,$2,$3,$4,$5,$6,$7,$8,$9,$2,$3,0*\n1,1,$1+$1,$2,$3*\n20,1,$1+$1,$2,$3,$4,$5,0*%\n%ADD12RoundRect,0.200000X0.275000X-0.200000X0.275000X0.200000X-0.275000X0.200000X-0.275000X-0.200000X0*%\nD12*\nX1000000Y2000000D03*\nM02*\n";
        let image = read(text).unwrap();
        assert_eq!(image.ops.len(), 3);
        match &image.ops[1] {
            Op::Disk { center, radius, dark } => {
                assert!((center.0 - 1_275_000.0).abs() < 1e-6 && (center.1 - 1_800_000.0).abs() < 1e-6);
                assert!((radius - 200_000.0).abs() < 1e-6 && *dark);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn regions_arcs_and_polarity() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.100000*%\nG75*\n%LPC*%\nG36*\nX0Y0D02*\nX1000000Y0D01*\nG03*\nX0Y1000000I-500000J500000D01*\nG01*\nX0Y0D01*\nG37*\n%LPD*%\nD10*\nX0Y0D02*\nX0Y5000000D01*\nM02*\n";
        let image = read(text).unwrap();
        match &image.ops[0] {
            Op::Polygon { points, dark } => {
                assert!(!dark);
                assert!(points.len() > 6);
                // The counter-clockwise arc bulges away from the origin.
                assert!(points.iter().any(|p| p.0 > 900_000.0 && p.1 > 900_000.0));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(image.ops[1], Op::Capsule { dark: true, .. }));
    }
}

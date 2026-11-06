//! The fabrication files of a [`Board`]: one Gerber per plotted layer,
//! plated and non-plated Excellon drills, an IPC-D-356A netlist and a
//! component position CSV. Layer content follows KiCad's plot of the same
//! board with `--subtract-soldermask` and absolute drill origin: mask files
//! are negative (openings dark), silkscreen loses the mask openings of its
//! side, and NPTH pads no larger than their hole carry no copper.

use crate::board::{Board, Drawing, Footprint, Layer, Pad, PadKind, PadShape, Result, Shape, Side};
use crate::font;
use crate::geometry::{Contour, Edge, Point, normalize_degrees, quarter_turns, rotate};
use crate::gerber::{Gerber, Template, field};

/// Pad geometry as a flash or a region, already placed on the board.
enum Art {
    Flash(Template, Point),
    Region(Contour),
}

#[derive(Clone, Copy)]
enum Outline {
    Circle,
    Rect,
    Oval,
    RoundRect(i64),
}

fn kiround(value: f64) -> i64 {
    value.round() as i64
}

fn corner_radius(pad: &Pad) -> i64 {
    match pad.shape {
        PadShape::RoundRect(ratio) => kiround(pad.width.min(pad.height) as f64 * ratio),
        _ => 0,
    }
}

fn outline(pad: &Pad, radius: i64) -> Outline {
    match pad.shape {
        PadShape::Circle => Outline::Circle,
        PadShape::Rect => Outline::Rect,
        PadShape::Oval => Outline::Oval,
        PadShape::RoundRect(_) => Outline::RoundRect(radius),
    }
}

fn place(center: Point, angle: f64, local: Point) -> Point {
    center.plus(rotate(local, angle))
}

/// The art of an outline of `width` x `height` (pad-local) turned by `angle`.
fn art(shape: Outline, width: i64, height: i64, angle: f64, center: Point) -> Option<Art> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let turns = quarter_turns(angle);
    let swapped = |w: i64, h: i64| if turns.is_some_and(|turns| turns % 2 == 1) { (h, w) } else { (w, h) };
    Some(match shape {
        Outline::Circle => Art::Flash(Template::Circle(width), center),
        Outline::Rect => match turns {
            Some(_) => {
                let (w, h) = swapped(width, height);
                Art::Flash(Template::Rect(w, h), center)
            }
            None => {
                let (hw, hh) = (width / 2, height / 2);
                let corners = [Point::new(-hw, -hh), Point::new(hw, -hh), Point::new(hw, hh), Point::new(-hw, hh)];
                Art::Region(Contour::polygon(&corners.map(|corner| place(center, angle, corner))))
            }
        },
        Outline::Oval if width == height => Art::Flash(Template::Circle(width), center),
        Outline::Oval => match turns {
            Some(_) => {
                let (w, h) = swapped(width, height);
                Art::Flash(Template::Obround(w, h), center)
            }
            None => {
                let horizontal = width > height;
                let (long, short) = if horizontal { (width, height) } else { (height, width) };
                let (a, r) = ((long - short) / 2, short / 2);
                let axis = |u: i64, v: i64| if horizontal { Point::new(u, v) } else { Point::new(v, u) };
                let map = |p: Point| place(center, angle, p);
                Art::Region(Contour {
                    start: map(axis(a, -r)),
                    edges: vec![
                        Edge::Line { end: map(axis(-a, -r)) },
                        Edge::Arc { mid: map(axis(-a - r, 0)), end: map(axis(-a, r)) },
                        Edge::Line { end: map(axis(a, r)) },
                        Edge::Arc { mid: map(axis(a + r, 0)), end: map(axis(a, -r)) },
                    ],
                })
            }
        },
        Outline::RoundRect(radius) if radius <= 0 => return art(Outline::Rect, width, height, angle, center),
        Outline::RoundRect(radius) if 2 * radius >= width.min(height) => {
            return art(Outline::Oval, width, height, angle, center);
        }
        Outline::RoundRect(radius) => {
            let (cx, cy) = (width / 2 - radius, height / 2 - radius);
            let corners = [Point::new(cx, -cy), Point::new(cx, cy), Point::new(-cx, cy), Point::new(-cx, -cy)];
            Art::Flash(Template::RoundRect(2 * radius, corners.map(|corner| rotate(corner, angle))), center)
        }
    })
}

fn draw_art(gerber: &mut Gerber, art: Art) {
    match art {
        Art::Flash(template, at) => gerber.flash(template, at),
        Art::Region(contour) => gerber.region(&contour),
    }
}

fn mask_art(board: &Board, footprint: &Footprint, pad: &Pad) -> Option<Art> {
    let margin = pad.mask_margin.or(footprint.mask_margin).unwrap_or(board.setup.mask_margin);
    let radius = (corner_radius(pad) + margin).max(0);
    let (width, height) = match pad.shape {
        PadShape::Circle => (pad.width + 2 * margin, pad.width + 2 * margin),
        _ => (pad.width + 2 * margin, pad.height + 2 * margin),
    };
    art(outline(pad, radius), width, height, pad.angle, pad.center)
}

fn paste_art(board: &Board, footprint: &Footprint, pad: &Pad) -> Option<Art> {
    let margin = pad.paste_margin.or(footprint.paste_margin).unwrap_or(board.setup.paste_margin);
    let ratio = pad.paste_ratio.or(footprint.paste_ratio).unwrap_or(board.setup.paste_ratio);
    let (mx, my) = (margin + kiround(pad.width as f64 * ratio), margin + kiround(pad.height as f64 * ratio));
    let radius = (corner_radius(pad) + mx.min(my)).max(0);
    let (width, height) = match pad.shape {
        PadShape::Circle => (pad.width + 2 * mx, pad.width + 2 * mx),
        _ => (pad.width + 2 * mx, pad.height + 2 * my),
    };
    art(outline(pad, radius), width, height, pad.angle, pad.center)
}

fn copper_art(pad: &Pad) -> Option<Art> {
    if pad.kind == PadKind::NonPlated
        && matches!(pad.shape, PadShape::Circle | PadShape::Oval)
        && pad.drill.is_some_and(|(width, height)| pad.width <= width && pad.height <= height)
    {
        return None;
    }
    art(outline(pad, corner_radius(pad)), pad.width, pad.height, pad.angle, pad.center)
}

fn drawing(gerber: &mut Gerber, drawing: &Drawing, outline_only: bool) {
    let width = drawing.width;
    let filled = drawing.filled && !outline_only;
    match &drawing.shape {
        Shape::Line { start, end } if width > 0 => gerber.line(width, *start, *end),
        Shape::Arc { start, mid, end } if width > 0 => gerber.arc(width, *start, *mid, *end),
        Shape::Circle { center, radius } if filled => gerber.flash(Template::Circle(2 * radius + width), *center),
        Shape::Circle { center, radius } if width > 0 => gerber.circle(width, *center, *radius),
        Shape::Polygon(contour) => {
            if filled {
                gerber.region(contour);
            }
            if width > 0 {
                gerber.outline(width, contour);
            }
        }
        _ => {}
    }
}

fn texts(gerber: &mut Gerber, board: &Board, layer: Layer) -> Result<()> {
    for text in board.texts.iter().filter(|text| text.layer == layer) {
        let (strokes, pen) = font::strokes(text)?;
        for stroke in strokes {
            let points: Vec<Point> = stroke.iter().map(|&(x, y)| Point::new(kiround(x), kiround(y))).collect();
            for pair in points.windows(2) {
                gerber.line(pen, pair[0], pair[1]);
            }
            if points.len() == 1 {
                gerber.line(pen, points[0], points[0]);
            }
        }
    }
    Ok(())
}

fn graphics(gerber: &mut Gerber, board: &Board, layer: Layer, outline_only: bool) -> Result<()> {
    for fill in board.fills.iter().filter(|fill| fill.layer == layer) {
        gerber.region(&fill.contour);
    }
    for item in board.drawings.iter().filter(|item| item.layer == layer) {
        drawing(gerber, item, outline_only);
    }
    texts(gerber, board, layer)
}

fn net_attribute(net: &Option<String>) -> (String, String) {
    ("N".to_owned(), field(net.as_deref().unwrap_or("N/C")))
}

fn copper(board: &Board, gerber: &mut Gerber, index: usize) -> Result<()> {
    let layer = Layer::Copper(index);
    gerber.set_function(Some("Conductor"));
    for fill in board.fills.iter().filter(|fill| fill.layer == layer) {
        gerber.set_attributes(vec![net_attribute(&fill.net)]);
        gerber.region(&fill.contour);
    }
    for track in board.tracks.iter().filter(|track| track.layer == index) {
        gerber.set_attributes(vec![net_attribute(&track.net)]);
        match track.path {
            crate::board::TrackPath::Line { start, end } => gerber.line(track.width, start, end),
            crate::board::TrackPath::Arc { start, mid, end } => gerber.arc(track.width, start, mid, end),
        }
    }
    gerber.set_function(Some("ViaPad"));
    for via in &board.vias {
        gerber.set_attributes(vec![net_attribute(&via.net)]);
        gerber.flash(Template::Circle(via.size), via.center);
    }
    for (footprint, pad) in board.pads().filter(|(_, pad)| pad.layers.has(layer)) {
        let Some(art) = copper_art(pad) else { continue };
        gerber.set_function(Some(match pad.kind {
            PadKind::Smd => "SMDPad,CuDef",
            PadKind::ThroughHole => "ComponentPad",
            PadKind::NonPlated => "WasherPad",
        }));
        let mut attributes = vec![("P".to_owned(), format!("{},{}", field(&footprint.reference), field(&pad.number)))];
        if pad.kind != PadKind::NonPlated {
            attributes.push(net_attribute(&pad.net));
        }
        attributes.push(("C".to_owned(), field(&footprint.reference)));
        gerber.set_attributes(attributes);
        draw_art(gerber, art);
    }
    gerber.set_attributes(Vec::new());
    gerber.set_function(Some("NonConductor"));
    for item in board.drawings.iter().filter(|item| item.layer == layer) {
        drawing(gerber, item, false);
    }
    texts(gerber, board, layer)
}

/// Mask openings of `side`, dark.
fn mask(board: &Board, gerber: &mut Gerber, side: Side) -> Result<()> {
    for (footprint, pad) in board.pads().filter(|(_, pad)| pad.layers.has(Layer::Mask(side))) {
        if let Some(art) = mask_art(board, footprint, pad) {
            draw_art(gerber, art);
        }
    }
    for via in board.vias.iter().filter(|via| !board.via_tented(via, side)) {
        gerber.flash(Template::Circle(via.size + 2 * board.setup.mask_margin), via.center);
    }
    graphics(gerber, board, Layer::Mask(side), false)
}

fn file_function(board: &Board, layer: Layer) -> (String, bool) {
    let side = |side: Side| if side == Side::Front { "Top" } else { "Bot" };
    match layer {
        Layer::Copper(index) => {
            let position = match index {
                0 => "Top",
                index if index + 1 == board.copper_count() => "Bot",
                _ => "Inr",
            };
            (format!("Copper,L{},{position}", index + 1), false)
        }
        Layer::Mask(s) => (format!("Soldermask,{}", side(s)), true),
        Layer::Paste(s) => (format!("Paste,{}", side(s)), false),
        Layer::Silk(s) => (format!("Legend,{}", side(s)), false),
        Layer::EdgeCuts => ("Profile,NP".to_owned(), false),
    }
}

/// The Gerber of one layer.
pub fn gerber(board: &Board, layer: Layer, version: &str) -> Result<String> {
    let (function, negative) = file_function(board, layer);
    let mut gerber = Gerber::new(version, &function, negative);
    match layer {
        Layer::Copper(index) => copper(board, &mut gerber, index)?,
        Layer::Mask(side) => mask(board, &mut gerber, side)?,
        Layer::Paste(side) => {
            for (footprint, pad) in board.pads().filter(|(_, pad)| pad.layers.has(Layer::Paste(side))) {
                if let Some(art) = paste_art(board, footprint, pad) {
                    draw_art(&mut gerber, art);
                }
            }
            graphics(&mut gerber, board, layer, false)?;
        }
        Layer::Silk(side) => {
            graphics(&mut gerber, board, layer, false)?;
            // --subtract-soldermask: no legend ink on exposed copper.
            gerber.set_dark(false);
            mask(board, &mut gerber, side)?;
            gerber.set_dark(true);
        }
        Layer::EdgeCuts => {
            gerber.set_function(Some("Profile"));
            graphics(&mut gerber, board, layer, true)?;
        }
    }
    Ok(gerber.finish())
}

/// The Gerber file name suffix of a layer, e.g. `F_Cu` or `F_Silkscreen`.
pub fn layer_file_name(board: &Board, layer: Layer) -> String {
    board.layer_name(layer).replace('.', "_")
}

// ---------------------------------------------------------------------------
// drills

#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    pub plated: bool,
    pub function: &'static str,
    pub diameter: i64,
    pub start: Point,
    /// The other end of a slot.
    pub end: Option<Point>,
}

pub fn holes(board: &Board) -> Vec<Hole> {
    let mut holes: Vec<Hole> = board
        .vias
        .iter()
        .map(|via| Hole { plated: true, function: "ViaDrill", diameter: via.drill, start: via.center, end: None })
        .collect();
    for (_, pad) in board.pads() {
        let Some((width, height)) = pad.drill else { continue };
        let plated = pad.kind == PadKind::ThroughHole;
        let (diameter, slot) = if width == height {
            (width, None)
        } else if width > height {
            (height, Some(Point::new((width - height) / 2, 0)))
        } else {
            (width, Some(Point::new(0, (height - width) / 2)))
        };
        let (start, end) = match slot {
            None => (pad.center, None),
            Some(half) => {
                let offset = rotate(half, pad.angle);
                (pad.center.minus(offset), Some(pad.center.plus(offset)))
            }
        };
        holes.push(Hole { plated, function: "ComponentDrill", diameter, start, end });
    }
    holes
}

/// Millimetres trimmed to at most six and at least `min` decimals.
fn decimal(nm: i64, min: usize) -> String {
    let text = crate::gerber::mm(nm);
    let (whole, fraction) = text.split_once('.').expect("six decimals");
    let trimmed = fraction.trim_end_matches('0');
    let fraction = if trimmed.len() < min { &fraction[..min] } else { trimmed };
    format!("{whole}.{fraction}")
}

fn excellon_xy(point: Point) -> String {
    format!("X{}Y{}", decimal(point.x, 1), decimal(-point.y, 1))
}

/// The Excellon file of the plated or non-plated holes.
pub fn excellon(board: &Board, plated: bool, version: &str) -> String {
    let (kind, short) = if plated { ("Plated", "PTH") } else { ("NonPlated", "NPTH") };
    let mut holes: Vec<Hole> = holes(board).into_iter().filter(|hole| hole.plated == plated).collect();
    let order = |function: &str| if function == "ViaDrill" { 0 } else { 1 };
    holes.sort_by(|a, b| {
        (a.diameter, order(a.function), a.end.is_some(), a.start, a.end).cmp(&(
            b.diameter,
            order(b.function),
            b.end.is_some(),
            b.start,
            b.end,
        ))
    });
    let mut tools: Vec<(i64, &'static str)> = Vec::new();
    for hole in &holes {
        if !tools.contains(&(hole.diameter, hole.function)) {
            tools.push((hole.diameter, hole.function));
        }
    }
    let mut lines = vec![
        "M48".to_owned(),
        format!("; DRILL file CopperScript {version}"),
        "; FORMAT={-:-/ absolute / metric / decimal}".to_owned(),
        format!("; #@! TF.GenerationSoftware,CopperScript,copper,{version}"),
        format!("; #@! TF.FileFunction,{kind},1,{},{short}", board.copper_count()),
        "FMAT,2".to_owned(),
        "METRIC".to_owned(),
    ];
    for (index, (diameter, function)) in tools.iter().enumerate() {
        lines.push(format!("; #@! TA.AperFunction,{kind},{short},{function}"));
        lines.push(format!("T{}C{}", index + 1, decimal(*diameter, 3)));
    }
    lines.extend(["%".to_owned(), "G90".to_owned(), "G05".to_owned()]);
    for (index, tool) in tools.iter().enumerate() {
        lines.push(format!("T{}", index + 1));
        for hole in holes.iter().filter(|hole| (hole.diameter, hole.function) == *tool) {
            match hole.end {
                None => lines.push(excellon_xy(hole.start)),
                Some(end) => {
                    lines.push(format!("{}G85{}", excellon_xy(hole.start), excellon_xy(end)));
                    lines.push("G05".to_owned());
                }
            }
        }
    }
    lines.push("M30".to_owned());
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

// ---------------------------------------------------------------------------
// IPC-D-356A

/// Nanometres in IPC-D-356 imperial units of 0.0001 inch, clamped.
fn decimil(nm: i64, clamp: i64) -> i64 {
    kiround(nm as f64 / 2540.0).clamp(-clamp, clamp)
}

fn d356_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_graphic() { c.to_ascii_uppercase() } else { '?' }).collect()
}

struct TestPoint {
    net: Option<String>,
    reference: String,
    pin: String,
    code: u32,
    drill: Option<(i64, bool)>,
    access: usize,
    center: Point,
    size: (i64, i64),
    rotation: i64,
    mask: u8,
}

/// The IPC-D-356A bare-board netlist: one record per via and copper pad.
pub fn ipc356(board: &Board, job: &str) -> String {
    let count = board.copper_count();
    let mut points: Vec<TestPoint> = Vec::new();
    for via in &board.vias {
        let mask = u8::from(board.via_tented(via, Side::Front)) | (u8::from(board.via_tented(via, Side::Back)) << 1);
        points.push(TestPoint {
            net: via.net.clone(),
            reference: "VIA".to_owned(),
            pin: String::new(),
            code: 317,
            drill: Some((via.drill, true)),
            access: 0,
            center: via.center,
            size: (via.size, 0),
            rotation: 0,
            mask,
        });
    }
    for (footprint, pad) in board.pads() {
        let (front, back) = (pad.layers.has(Layer::Copper(0)), pad.layers.has(Layer::Copper(count - 1)));
        let access = match (front, back) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => count,
            _ => match pad.layers.copper.iter().position(|&on| on) {
                Some(index) => index + 1,
                None => continue,
            },
        };
        let mask = u8::from(!pad.layers.has(Layer::Mask(Side::Front)))
            | (u8::from(!pad.layers.has(Layer::Mask(Side::Back))) << 1);
        points.push(TestPoint {
            net: pad.net.clone(),
            reference: footprint.reference.clone(),
            pin: pad.number.clone(),
            code: match pad.kind {
                PadKind::Smd => 327,
                PadKind::ThroughHole => 317,
                PadKind::NonPlated => 367,
            },
            drill: pad.drill.map(|(width, height)| (width.min(height), pad.kind != PadKind::NonPlated)),
            access,
            center: pad.center,
            size: (pad.width, if pad.shape == PadShape::Circle { 0 } else { pad.height }),
            rotation: normalize_degrees(-pad.angle) as i64,
            mask,
        });
    }
    // Net names longer than the 14-column field, or ambiguous once
    // upper-cased, are replaced by aliases declared in NNAME records.
    let mut names: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    let mut nets: Vec<&String> = points.iter().filter_map(|point| point.net.as_ref()).collect();
    nets.sort();
    nets.dedup();
    let mut canonical: indexmap::IndexMap<String, usize> = indexmap::IndexMap::new();
    for net in &nets {
        *canonical.entry(d356_name(net)).or_default() += 1;
    }
    let mut lines = vec![
        format!("P  JOB   {job}"),
        "P  CODE  00".to_owned(),
        "P  UNITS CUST 0".to_owned(),
        "P  VER   IPC-D-356A".to_owned(),
        "P  IMAGE PRIMARY".to_owned(),
    ];
    let mut alias = 0;
    for net in nets {
        let candidate = d356_name(net);
        if candidate.len() <= 14 && candidate != "N/C" && canonical[&candidate] == 1 {
            names.insert(net.clone(), candidate);
            continue;
        }
        let name = loop {
            alias += 1;
            let name = format!("NNAME{alias}");
            if !canonical.contains_key(&name) {
                break name;
            }
        };
        let full: String = d356_name(net).chars().take(66).collect();
        lines.push(format!("P  {name} {full}"));
        names.insert(net.clone(), name);
    }
    let signal = |point: &TestPoint| point.net.as_ref().map_or("N/C".to_owned(), |net| names[net].clone());
    points.sort_by(|a, b| (signal(a), &a.reference, &a.pin).cmp(&(signal(b), &b.reference, &b.pin)));
    for point in &points {
        let pin_mark = if point.pin.is_empty() { ' ' } else { '-' };
        // Vias are net midpoints; every pad, numbered or not, is a terminal.
        let midpoint = if point.reference == "VIA" && point.pin.is_empty() { 'M' } else { ' ' };
        let hole = match point.drill {
            Some((drill, plated)) => format!("D{:04}{}", decimil(drill, 9999), if plated { 'P' } else { 'U' }),
            None => "      ".to_owned(),
        };
        lines.push(format!(
            "{:03}{:<14.14}   {:<6.6}{pin_mark}{:<4.4}{midpoint}{hole}A{:02}X{:+07}Y{:+07}X{:04}Y{:04}R{:03} S{}",
            point.code,
            signal(point),
            d356_name(&point.reference),
            d356_name(&point.pin),
            point.access,
            decimil(point.center.x, 999_999),
            decimil(-point.center.y, 999_999),
            decimil(point.size.0, 9999),
            decimil(point.size.1, 9999),
            point.rotation,
            point.mask,
        ));
    }
    lines.push("999".to_owned());
    lines.iter().map(|line| line.chars().take(80).collect::<String>().trim_end().to_owned() + "\n").collect()
}

// ---------------------------------------------------------------------------
// positions

/// `A10` sorts after `A9`: digit runs compare numerically.
fn natural_key(text: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut chars = text.chars().peekable();
    while chars.peek().is_some() {
        let mut word = String::new();
        while let Some(&c) = chars.peek().filter(|c| !c.is_ascii_digit()) {
            word.push(c);
            chars.next();
        }
        let mut digits = String::new();
        while let Some(&c) = chars.peek().filter(|c| c.is_ascii_digit()) {
            digits.push(c);
            chars.next();
        }
        key.push((word, digits.parse().unwrap_or(0)));
    }
    key
}

fn csv_field(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// Component positions in KiCad's CSV columns: millimetres, Y up, both
/// sides, footprint origins and angles; DNP and excluded footprints omitted.
pub fn positions(board: &Board) -> String {
    let mut footprints: Vec<&Footprint> =
        board.footprints.iter().filter(|footprint| !footprint.exclude_from_pos_files && !footprint.dnp).collect();
    footprints.sort_by_key(|footprint| natural_key(&footprint.reference));
    let mut text = "Ref,Val,Package,PosX,PosY,Rot,Side\n".to_owned();
    for footprint in footprints {
        let rotation = normalize_degrees(footprint.angle);
        text.push_str(&format!(
            "{},{},{},{},{},{:.6},{}\n",
            csv_field(&footprint.reference),
            csv_field(&footprint.value),
            csv_field(&footprint.package),
            crate::gerber::mm(footprint.position.x),
            crate::gerber::mm(-footprint.position.y),
            rotation,
            if footprint.side == Side::Front { "top" } else { "bottom" },
        ));
    }
    text
}

/// Every file of the board: `(file name, contents)`.
pub fn plot(board: &Board, stem: &str, version: &str) -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    for layer in board.layers() {
        files.push((format!("{stem}-{}.gbr", layer_file_name(board, layer)), gerber(board, layer, version)?));
    }
    files.push((format!("{stem}-PTH.drl"), excellon(board, true, version)));
    files.push((format!("{stem}-NPTH.drl"), excellon(board, false, version)));
    files.push((format!("{stem}.d356"), ipc356(board, stem)));
    files.push(("positions.csv".to_owned(), positions(board)));
    Ok(files)
}

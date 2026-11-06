//! SVG illustrations of plotted Gerbers: each layer is read back with the
//! comparison reader and drawn as a coloured plate, so a picture shows the
//! fabrication artwork itself rather than the board model that produced it.
//!
//! A layer is a mask built from its image objects in file order (dark
//! objects add, clear objects remove), filled with one colour. A layer can
//! be cut to the image of another (`F.Cu&F.Mask`: top copper inside the
//! solder-mask openings, i.e. exposed copper). Gerber Y points up; the SVG
//! is in millimetres with Y down, viewed from above and never mirrored.

use std::fmt::Write as _;

use indexmap::IndexMap;

use crate::compare::gerber::{Image, Op};

pub type Result<T> = std::result::Result<T, String>;

/// The KiCad layer name of a Gerber `TF.FileFunction` value, e.g.
/// `Copper,L3,Inr` -> `In2.Cu`.
pub fn layer_name(file_function: &str) -> Option<String> {
    let parts: Vec<&str> = file_function.split(',').collect();
    let side = |side: &str| match side {
        "Top" => Some("F"),
        "Bot" => Some("B"),
        _ => None,
    };
    match parts.as_slice() {
        ["Copper", level, position, ..] => {
            let index: usize = level.strip_prefix('L')?.parse().ok()?;
            match *position {
                "Top" => Some("F.Cu".to_owned()),
                "Bot" => Some("B.Cu".to_owned()),
                "Inr" if index > 1 => Some(format!("In{}.Cu", index - 1)),
                _ => None,
            }
        }
        ["Soldermask", s, ..] => Some(format!("{}.Mask", side(s)?)),
        ["Paste", s, ..] => Some(format!("{}.Paste", side(s)?)),
        ["Legend", s, ..] => Some(format!("{}.Silkscreen", side(s)?)),
        ["Profile", ..] => Some("Edge.Cuts".to_owned()),
        _ => None,
    }
}

/// KiCad's default colour for a layer, used when a plate names none.
pub fn default_color(layer: &str) -> &'static str {
    match layer {
        "F.Cu" => "#c83434",
        "B.Cu" => "#4d7fc4",
        "In1.Cu" => "#7fc87f",
        "In2.Cu" => "#ce7d2c",
        "In3.Cu" => "#4fcbcb",
        "In4.Cu" => "#db628b",
        "F.Silkscreen" => "#f2eda1",
        "B.Silkscreen" => "#e8b2a7",
        "F.Mask" | "B.Mask" => "#d864ff",
        "F.Paste" | "B.Paste" => "#b4a0a0",
        "Edge.Cuts" => "#d0d2cd",
        _ => "#c8c8c8",
    }
}

/// Every Gerber directly in `directory`, by layer name. Files whose
/// `TF.FileFunction` names no drawable layer are skipped; two files of one
/// layer are an error.
pub fn load(directory: &std::path::Path) -> Result<IndexMap<String, Image>> {
    let shown = directory.display();
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
        .map_err(|failure| format!("cannot read {shown}: {failure}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("gbr")))
        .collect();
    entries.sort();
    let mut images = IndexMap::new();
    let mut files: IndexMap<String, String> = IndexMap::new();
    for path in entries {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let text = std::fs::read_to_string(&path).map_err(|failure| format!("cannot read {name}: {failure}"))?;
        let image = crate::compare::gerber::read(&text).map_err(|failure| format!("{name}: {failure}"))?;
        let Some(layer) = image.file_function.as_deref().and_then(layer_name) else { continue };
        if let Some(other) = files.insert(layer.clone(), name.clone()) {
            return Err(format!("{other} and {name} both plot {layer}"));
        }
        images.insert(layer, image);
    }
    if images.is_empty() {
        return Err(format!("no Gerber layers in {shown}"));
    }
    Ok(images)
}

/// One coloured layer of the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub layer: String,
    /// Draw only where this layer's image is dark.
    pub within: Option<String>,
    pub color: String,
    pub opacity: f64,
}

impl Plate {
    /// `LAYER[&WITHIN][:COLOR[:OPACITY]]`, e.g. `F.Cu&F.Mask:#d9a441:0.9`.
    pub fn parse(spec: &str) -> Result<Plate> {
        let mut parts = spec.split(':');
        let names = parts.next().unwrap_or_default();
        let (layer, within) = match names.split_once('&') {
            Some((layer, within)) => (layer.to_owned(), Some(within.to_owned())),
            None => (names.to_owned(), None),
        };
        if layer.is_empty() || within.as_deref() == Some("") {
            return Err(format!("layer missing in {spec:?}"));
        }
        let color = match parts.next() {
            Some(color) if valid_color(color) => color.to_owned(),
            Some(color) => return Err(format!("colour must be #rgb or #rrggbb, not {color:?}")),
            None => default_color(&layer).to_owned(),
        };
        let opacity = match parts.next() {
            Some(value) => match value.parse::<f64>() {
                Ok(opacity) if (0.0..=1.0).contains(&opacity) => opacity,
                _ => return Err(format!("opacity must be between 0 and 1, not {value:?}")),
            },
            None => 1.0,
        };
        if parts.next().is_some() {
            return Err(format!("too many ':' fields in {spec:?}"));
        }
        Ok(Plate { layer, within, color, opacity })
    }
}

fn valid_color(color: &str) -> bool {
    color.strip_prefix('#').is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// How the picture is framed and backed.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Colour under every plate, inside the board profile's bounds.
    pub background: Option<String>,
    /// Space around the profile, in millimetres.
    pub margin_mm: f64,
    pub title: String,
}

/// Millimetres from Gerber nanometres, Y flipped to point down.
fn x(value: f64) -> String {
    number(value / 1e6)
}
fn y(value: f64) -> String {
    number(-value / 1e6)
}
fn number(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The objects of `image` as mask content: dark white, clear black.
fn mask_body(image: &Image, svg: &mut String) {
    for op in &image.ops {
        let ink = |dark: bool| if dark { "#fff" } else { "#000" };
        match op {
            Op::Disk { center, radius, dark } => {
                let _ = write!(
                    svg,
                    r##"<circle cx="{}" cy="{}" r="{}" fill="{}"/>"##,
                    x(center.0),
                    y(center.1),
                    number(radius / 1e6),
                    ink(*dark)
                );
            }
            Op::Polygon { points, dark } => {
                if points.len() < 3 {
                    continue;
                }
                svg.push_str(r##"<path d="M"##);
                for (index, point) in points.iter().enumerate() {
                    if index > 0 {
                        svg.push('L');
                    }
                    let _ = write!(svg, "{} {}", x(point.0), y(point.1));
                }
                let _ = write!(svg, r##"Z" fill="{}"/>"##, ink(*dark));
            }
            Op::Capsule { a, b, width, dark } => {
                let _ = write!(
                    svg,
                    r##"<path d="M{} {}L{} {}" stroke="{}" stroke-width="{}" stroke-linecap="round" fill="none"/>"##,
                    x(a.0),
                    y(a.1),
                    x(b.0),
                    y(b.1),
                    ink(*dark),
                    number(width / 1e6)
                );
            }
        }
    }
}

/// An SVG picture of `plates`, drawn bottom first, from the images named by
/// layer. The frame is the bounds of `Edge.Cuts` when present, else of
/// every image drawn.
pub fn svg(images: &IndexMap<String, Image>, plates: &[Plate], options: &Options) -> Result<String> {
    let image = |name: &str| {
        images.get(name).ok_or_else(|| {
            format!("no plotted {name} layer; available: {}", images.keys().cloned().collect::<Vec<_>>().join(", "))
        })
    };
    let mut used: Vec<&str> = Vec::new();
    for plate in plates {
        for name in std::iter::once(plate.layer.as_str()).chain(plate.within.as_deref()) {
            image(name)?;
            if !used.contains(&name) {
                used.push(name);
            }
        }
    }
    if plates.is_empty() {
        return Err("no layers to draw".to_owned());
    }
    let framing: Vec<&Image> = match images.get("Edge.Cuts") {
        Some(edge) if edge.bounds().is_some() => vec![edge],
        _ => used.iter().map(|name| &images[*name]).collect(),
    };
    let bounds = framing
        .iter()
        .filter_map(|image| image.bounds())
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
    let Some((x0, y0, x1, y1)) = bounds else { return Err("nothing to draw: every layer is empty".to_owned()) };
    let margin = options.margin_mm * 1e6;
    let (left, top) = (x0 - margin, y1 + margin);
    let (width, height) = (x1 - x0 + 2.0 * margin, y1 - y0 + 2.0 * margin);

    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="{l} {t} {w} {h}">"##,
        w = number(width / 1e6),
        h = number(height / 1e6),
        l = x(left),
        t = y(top),
    );
    let _ = write!(svg, "<title>{}</title>", escape(&options.title));
    svg.push_str("<desc>Native CopperScript plot (copper render): Gerber artwork read back and drawn layer by layer, viewed from above.</desc><defs>");
    let full = format!(
        r##"<rect x="{}" y="{}" width="{}" height="{}""##,
        x(left),
        y(top),
        number(width / 1e6),
        number(height / 1e6)
    );
    for (index, name) in used.iter().enumerate() {
        let _ = write!(svg, r##"<mask id="m{index}" maskUnits="userSpaceOnUse">{full} fill="#000"/>"##);
        mask_body(&images[*name], &mut svg);
        svg.push_str("</mask>");
    }
    svg.push_str("</defs>");
    if let Some(background) = &options.background {
        let _ = write!(
            svg,
            r##"<rect x="{}" y="{}" width="{}" height="{}" fill="{background}"/>"##,
            x(x0),
            y(y1),
            number((x1 - x0) / 1e6),
            number((y1 - y0) / 1e6)
        );
    }
    let mask = |name: &str| used.iter().position(|used| *used == name).expect("collected above");
    for plate in plates {
        let opacity =
            if plate.opacity < 1.0 { format!(r##" opacity="{}""##, number(plate.opacity)) } else { String::new() };
        let _ = write!(svg, r##"<g data-layer="{}"{opacity}>"##, escape(&plate.layer));
        if let Some(within) = &plate.within {
            let _ = write!(svg, r##"<g mask="url(#m{})">"##, mask(within));
        }
        let _ = write!(svg, r##"{full} fill="{}" mask="url(#m{})"/>"##, plate.color, mask(&plate.layer));
        if plate.within.is_some() {
            svg.push_str("</g>");
        }
        svg.push_str("</g>");
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(ops: Vec<Op>) -> Image {
        Image { file_function: None, negative: false, ops }
    }

    #[test]
    fn names_layers_from_file_functions() {
        assert_eq!(layer_name("Copper,L1,Top").as_deref(), Some("F.Cu"));
        assert_eq!(layer_name("Copper,L3,Inr").as_deref(), Some("In2.Cu"));
        assert_eq!(layer_name("Copper,L6,Bot").as_deref(), Some("B.Cu"));
        assert_eq!(layer_name("Soldermask,Bot").as_deref(), Some("B.Mask"));
        assert_eq!(layer_name("Legend,Top").as_deref(), Some("F.Silkscreen"));
        assert_eq!(layer_name("Profile,NP").as_deref(), Some("Edge.Cuts"));
        assert_eq!(layer_name("Copper,L1,Inr"), None);
        assert_eq!(layer_name("Drillmap"), None);
    }

    #[test]
    fn parses_plate_specs() {
        assert_eq!(
            Plate::parse("F.Cu&F.Mask:#d9a441:0.5").unwrap(),
            Plate { layer: "F.Cu".into(), within: Some("F.Mask".into()), color: "#d9a441".into(), opacity: 0.5 }
        );
        assert_eq!(Plate::parse("B.Cu").unwrap().color, "#4d7fc4");
        assert!(Plate::parse("F.Cu:red").is_err());
        assert!(Plate::parse("F.Cu:#fff:2").is_err());
        assert!(Plate::parse("F.Cu&:#fff").is_err());
    }

    #[test]
    fn draws_dark_and_clear_objects_in_order_within_the_profile() {
        let mut images = IndexMap::new();
        images.insert(
            "F.Cu".to_owned(),
            image(vec![
                Op::Polygon { points: vec![(0.0, 0.0), (2e6, 0.0), (2e6, 1e6)], dark: true },
                Op::Disk { center: (1e6, 5e5), radius: 1e5, dark: false },
                Op::Capsule { a: (0.0, 0.0), b: (1e6, 0.0), width: 2e5, dark: true },
            ]),
        );
        images.insert(
            "Edge.Cuts".to_owned(),
            image(vec![Op::Capsule { a: (0.0, 0.0), b: (1e7, 1e7), width: 0.0, dark: true }]),
        );
        let plates = [Plate::parse("F.Cu").unwrap(), Plate::parse("F.Cu&Edge.Cuts:#000:0.25").unwrap()];
        let options = Options { background: Some("#123456".into()), margin_mm: 1.0, title: "a <b>".into() };
        let svg = svg(&images, &plates, &options).unwrap();
        assert!(svg.starts_with(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="12mm" height="12mm" viewBox="-1 -11 12 12">"##
        ));
        assert!(svg.contains("<title>a &lt;b&gt;</title>"));
        assert!(
            svg.contains(r##"<path d="M0 0L2 0L2 -1Z" fill="#fff"/><circle cx="1" cy="-0.5" r="0.1" fill="#000"/>"##)
        );
        assert!(svg.contains(r##"stroke="#fff" stroke-width="0.2""##));
        assert!(svg.contains(r##"<rect x="0" y="-10" width="10" height="10" fill="#123456"/>"##));
        assert!(svg.contains(r##"<g data-layer="F.Cu" opacity="0.25"><g mask="url(#m1)">"##));
        assert_eq!(svg.matches("<mask ").count(), 2);
    }

    #[test]
    fn refuses_a_missing_layer() {
        let images = IndexMap::new();
        let error = svg(
            &images,
            &[Plate::parse("In2.Cu").unwrap()],
            &Options { background: None, margin_mm: 0.0, title: String::new() },
        )
        .unwrap_err();
        assert!(error.starts_with("no plotted In2.Cu layer"));
    }
}

//! A Gerber X2 (RS-274X) writer: format 4.6 millimetres, so a coordinate is
//! the board position in nanometres with Y negated (Gerber Y points up).
//! Apertures are deduplicated per aperture function and listed before the
//! image; object attributes (`TO`) are emitted only when they change. Output
//! is deterministic: no creation date is written.

use indexmap::IndexMap;

use crate::geometry::{Contour, Edge, Point, circle_through, turns_positive};

/// An aperture template with its sizes in nanometres.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Template {
    Circle(i64),
    Rect(i64, i64),
    Obround(i64, i64),
    /// Corner-circle diameter and the four corner centres, relative to the flash.
    RoundRect(i64, [Point; 4]),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Aperture {
    template: Template,
    function: Option<&'static str>,
}

pub struct Gerber {
    header: Vec<String>,
    apertures: IndexMap<Aperture, u32>,
    body: Vec<String>,
    current_aperture: Option<u32>,
    position: Option<Point>,
    dark: bool,
    attributes: Vec<(String, String)>,
    function: Option<&'static str>,
}

/// Millimetres with six decimals.
pub fn mm(nm: i64) -> String {
    let sign = if nm < 0 { "-" } else { "" };
    format!("{sign}{}.{:06}", nm.unsigned_abs() / 1_000_000, nm.unsigned_abs() % 1_000_000)
}

/// An X2 attribute field: `%`, `*`, `,` and `\` escaped as `\uXXXX`.
pub fn field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '%' | '*' | ',' | '\\') || c.is_control() {
            escaped.push_str(&format!("\\u{:04X}", c as u32));
        } else {
            escaped.push(c);
        }
    }
    escaped
}

fn xy(point: Point) -> String {
    format!("X{}Y{}", point.x, -point.y)
}

impl Gerber {
    /// A new image with `%TF.FileFunction,<function>` and the given polarity.
    pub fn new(software_version: &str, file_function: &str, negative: bool) -> Self {
        let header = vec![
            format!("%TF.GenerationSoftware,CopperScript,copper,{}*%", field(software_version)),
            "%TF.SameCoordinates,Original*%".to_owned(),
            format!("%TF.FileFunction,{file_function}*%"),
            format!("%TF.FilePolarity,{}*%", if negative { "Negative" } else { "Positive" }),
            "%FSLAX46Y46*%".to_owned(),
            "G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*".to_owned(),
            "G04 Created by CopperScript*".to_owned(),
            "%MOMM*%".to_owned(),
            "%LPD*%".to_owned(),
            "G75*".to_owned(),
            "G01*".to_owned(),
        ];
        Self {
            header,
            apertures: IndexMap::new(),
            body: Vec::new(),
            current_aperture: None,
            position: None,
            dark: true,
            attributes: Vec::new(),
            function: None,
        }
    }

    /// The aperture function (`%TA.AperFunction`) of the following objects.
    pub fn set_function(&mut self, function: Option<&'static str>) {
        self.function = function;
    }

    /// The object attributes (`%TO`) of the following objects.
    pub fn set_attributes(&mut self, attributes: Vec<(String, String)>) {
        if attributes == self.attributes {
            return;
        }
        if !self.attributes.is_empty() {
            self.body.push("%TD*%".to_owned());
        }
        for (name, value) in &attributes {
            self.body.push(format!("%TO.{name},{value}*%"));
        }
        self.attributes = attributes;
    }

    /// Dark (`true`) or clear polarity for the following objects.
    pub fn set_dark(&mut self, dark: bool) {
        if dark != self.dark {
            self.body.push(if dark { "%LPD*%" } else { "%LPC*%" }.to_owned());
            self.dark = dark;
        }
    }

    fn select(&mut self, template: Template) {
        let aperture = Aperture { template, function: self.function };
        let next = 10 + self.apertures.len() as u32;
        let code = *self.apertures.entry(aperture).or_insert(next);
        if self.current_aperture != Some(code) {
            self.body.push(format!("D{code}*"));
            self.current_aperture = Some(code);
        }
    }

    fn move_to(&mut self, point: Point) {
        if self.position != Some(point) {
            self.body.push(format!("{}D02*", xy(point)));
        }
        self.position = Some(point);
    }

    pub fn flash(&mut self, template: Template, at: Point) {
        self.select(template);
        self.body.push(format!("{}D03*", xy(at)));
        self.position = Some(at);
    }

    /// A straight stroke with a round pen.
    pub fn line(&mut self, width: i64, start: Point, end: Point) {
        self.select(Template::Circle(width));
        self.move_to(start);
        self.body.push(format!("{}D01*", xy(end)));
        self.position = Some(end);
    }

    /// A connected polyline with a round pen.
    pub fn polyline(&mut self, width: i64, points: &[Point]) {
        let Some((&first, rest)) = points.split_first() else { return };
        self.select(Template::Circle(width));
        self.move_to(first);
        if rest.is_empty() {
            self.body.push(format!("{}D01*", xy(first)));
        }
        for &point in rest {
            self.body.push(format!("{}D01*", xy(point)));
        }
        self.position = points.last().copied();
    }

    fn arc_to(&mut self, start: Point, mid: Point, end: Point) {
        // Direction in Gerber's Y-up frame: negate Y before the turn test.
        let flip = |p: Point| Point::new(p.x, -p.y);
        let counter_clockwise = turns_positive(flip(start), flip(mid), flip(end));
        let Some(((cx, cy), _)) = circle_through(start, mid, end) else {
            self.body.push(format!("{}D01*", xy(end)));
            return;
        };
        let (i, j) = ((cx - start.x as f64).round() as i64, -((cy - start.y as f64).round() as i64));
        self.body.push(if counter_clockwise { "G03*" } else { "G02*" }.to_owned());
        self.body.push(format!("{}I{i}J{j}D01*", xy(end)));
        self.body.push("G01*".to_owned());
    }

    /// A three-point arc stroke with a round pen; equal ends draw a circle.
    pub fn arc(&mut self, width: i64, start: Point, mid: Point, end: Point) {
        self.select(Template::Circle(width));
        self.move_to(start);
        self.arc_to(start, mid, end);
        self.position = Some(end);
    }

    /// A full circle stroke.
    pub fn circle(&mut self, width: i64, center: Point, radius: i64) {
        let start = Point::new(center.x + radius, center.y);
        self.select(Template::Circle(width));
        self.move_to(start);
        let (i, j) = (-radius, 0);
        self.body.push("G02*".to_owned());
        self.body.push(format!("{}I{i}J{j}D01*", xy(start)));
        self.body.push("G01*".to_owned());
        self.position = Some(start);
    }

    /// The closed outline of `contour` as a stroke.
    pub fn outline(&mut self, width: i64, contour: &Contour) {
        self.select(Template::Circle(width));
        self.move_to(contour.start);
        let mut current = contour.start;
        for edge in &contour.edges {
            match *edge {
                Edge::Line { end } => self.body.push(format!("{}D01*", xy(end))),
                Edge::Arc { mid, end } => self.arc_to(current, mid, end),
            }
            current = edge.end();
        }
        self.position = Some(current);
    }

    /// A filled region (`G36`/`G37`).
    pub fn region(&mut self, contour: &Contour) {
        // A region takes the aperture attributes in effect at G36.
        if let Some(function) = self.function {
            self.body.push(format!("%TA.AperFunction,{function}*%"));
        }
        self.body.push("G36*".to_owned());
        self.body.push(format!("{}D02*", xy(contour.start)));
        let mut current = contour.start;
        for edge in &contour.edges {
            match *edge {
                Edge::Line { end } => self.body.push(format!("{}D01*", xy(end))),
                Edge::Arc { mid, end } => self.arc_to(current, mid, end),
            }
            current = edge.end();
        }
        self.body.push("G37*".to_owned());
        if self.function.is_some() {
            self.body.push("%TD.AperFunction*%".to_owned());
        }
        self.position = None;
    }

    /// The complete file.
    pub fn finish(mut self) -> String {
        if !self.attributes.is_empty() {
            self.body.push("%TD*%".to_owned());
        }
        let mut lines = self.header;
        lines.push("G04 APERTURE LIST*".to_owned());
        if self.apertures.keys().any(|aperture| matches!(aperture.template, Template::RoundRect(..))) {
            lines.extend(
                [
                    "%AMRoundRect*",
                    "0 Rectangle with rounded corners*",
                    "0 $1 corner diameter, $2..$9 corner centres*",
                    "4,1,4,$2,$3,$4,$5,$6,$7,$8,$9,$2,$3,0*",
                    "1,1,$1,$2,$3*",
                    "1,1,$1,$4,$5*",
                    "1,1,$1,$6,$7*",
                    "1,1,$1,$8,$9*",
                    "20,1,$1,$2,$3,$4,$5,0*",
                    "20,1,$1,$4,$5,$6,$7,0*",
                    "20,1,$1,$6,$7,$8,$9,0*",
                    "20,1,$1,$8,$9,$2,$3,0*%",
                ]
                .map(str::to_owned),
            );
        }
        for (aperture, code) in &self.apertures {
            if let Some(function) = aperture.function {
                lines.push(format!("%TA.AperFunction,{function}*%"));
            }
            let definition = match &aperture.template {
                Template::Circle(diameter) => format!("C,{}", mm(*diameter)),
                Template::Rect(width, height) => format!("R,{}X{}", mm(*width), mm(*height)),
                Template::Obround(width, height) => format!("O,{}X{}", mm(*width), mm(*height)),
                Template::RoundRect(diameter, corners) => {
                    let mut text = format!("RoundRect,{}", mm(*diameter));
                    for corner in corners {
                        text.push_str(&format!("X{}X{}", mm(corner.x), mm(-corner.y)));
                    }
                    text
                }
            };
            lines.push(format!("%ADD{code}{definition}*%"));
            if aperture.function.is_some() {
                lines.push("%TD*%".to_owned());
            }
        }
        lines.push("G04 APERTURE END LIST*".to_owned());
        lines.extend(self.body);
        lines.push("M02*".to_owned());
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_nanometres_with_y_up() {
        let mut gerber = Gerber::new("0.1.0", "Copper,L1,Top", false);
        gerber.set_function(Some("Conductor"));
        gerber.set_attributes(vec![("N".to_owned(), field("GND"))]);
        gerber.line(200_000, Point::new(1_000_000, 2_000_000), Point::new(3_000_000, 2_000_000));
        gerber.line(200_000, Point::new(3_000_000, 2_000_000), Point::new(3_000_000, 5_000_000));
        let text = gerber.finish();
        assert!(text.contains("%TA.AperFunction,Conductor*%\n%ADD10C,0.200000*%\n%TD*%"), "{text}");
        assert!(
            text.contains(
                "%TO.N,GND*%\nD10*\nX1000000Y-2000000D02*\nX3000000Y-2000000D01*\nX3000000Y-5000000D01*\n%TD*%\nM02*"
            ),
            "{text}"
        );
        assert!(!text.contains("CreationDate"));
    }

    #[test]
    fn fields_escape_reserved_characters() {
        assert_eq!(field("A,B*C%D\\E"), "A\\u002CB\\u002AC\\u0025D\\u005CE");
    }

    #[test]
    fn arcs_turn_in_the_gerber_frame() {
        let mut gerber = Gerber::new("0.1.0", "Profile,NP", false);
        // Board Y down: (0,-1) is above the centre, so start -> top -> end is
        // clockwise on screen and in Gerber's Y-up frame alike.
        gerber.arc(50_000, Point::new(-1_000_000, 0), Point::new(0, -1_000_000), Point::new(1_000_000, 0));
        let text = gerber.finish();
        assert!(text.contains("G02*\nX1000000Y0I1000000J0D01*\nG01*"), "{text}");
    }
}

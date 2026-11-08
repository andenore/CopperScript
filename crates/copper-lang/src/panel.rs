//! `panel` documents: already-routed boards placed on a fabrication panel,
//! with rails, mouse-bite tabs, fiducials and tooling holes. Lowering checks
//! the declaration and resolves units; `copper panelize` composes the boards
//! (see docs/panelization-plan.md). `preset = "jlcpcb";` fills the
//! properties a panel leaves out from a fab preset (see `panel_presets`).
//!
//! ```copper
//! panel AntennaPanel {
//!     spacing = 2 mm;
//!     rails = { top = 5 mm; bottom = 5 mm; };
//!     tab_width = 3 mm;
//!     mouse_bite = { drill = 0.5 mm; pitch = 0.75 mm; inset = 0.25 mm; };
//!     board A1 { source = "a1.copper"; pcb = "build/a1/a1.kicad_pcb"; at = (5 mm, 7 mm); }
//!     tab a1_left { board = A1; side = left; at = 30 mm; }
//! }
//! ```

use std::path::Path;

use indexmap::{IndexMap, IndexSet};
use num_traits::ToPrimitive;

use copper_units::{Decimal, Dimension, Quantity, Rounding};

use crate::error::CopperScriptError;
use crate::panel_presets::{self, Defaults};
use crate::syntax::{Declaration, Document, DocumentKind, Location, MacroItemDecl, Scalar, Value};

pub type Result<T> = std::result::Result<T, CopperScriptError>;

fn fail<T>(message: impl Into<String>, location: &Location) -> Result<T> {
    Err(CopperScriptError::new("PNL001", message, location.clone()))
}

/// A side of a rectangular board outline, in the board's own frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn parse(text: &str) -> Option<Side> {
        Some(match text {
            "left" => Side::Left,
            "right" => Side::Right,
            "top" => Side::Top,
            "bottom" => Side::Bottom,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
}

/// Rail widths in nanometres; zero means no rail on that side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rails {
    pub top: i64,
    pub bottom: i64,
    pub left: i64,
    pub right: i64,
}

/// Mouse-bite holes along each tab's board edge, in nanometres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseBite {
    pub drill: i64,
    /// Centre-to-centre spacing of the holes.
    pub pitch: i64,
    /// How far the hole centres sit inside the board edge.
    pub inset: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelBoard {
    pub name: String,
    pub location: Location,
    /// The board's `.copper` source, relative to the panel file.
    pub source: String,
    /// Its routed `.kicad_pcb`, relative to the panel file.
    pub pcb: String,
    /// Where the board's outline origin lands on the panel, in nanometres.
    pub at: [i64; 2],
    /// Counter-clockwise quarter turns (0 to 3).
    pub quarter_turns: u8,
}

/// Where a tab sits on its board: a rectangle side or a named mechanical edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabEdge {
    Side(Side),
    Named(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelTab {
    pub name: String,
    pub location: Location,
    pub board: String,
    pub edge: TabEdge,
    /// The tab centre's distance along the edge from its start, in nanometres.
    pub at: i64,
    pub width: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelSpec {
    pub name: String,
    pub location: Location,
    /// The milled gap between boards, and between boards and rails.
    pub spacing: i64,
    pub rails: Rails,
    pub tab_width: i64,
    pub mouse_bite: MouseBite,
    pub fiducials: u32,
    pub tooling_hole: Option<i64>,
    /// Fiducial centres' distance from the panel's outer edge; `None` centres
    /// them on their rail.
    pub fiducial_offset: Option<i64>,
    /// The fab preset named by the panel, if any.
    pub preset: Option<String>,
    /// Where each panel value came from: `explicit`, `preset NAME` or
    /// `default`, keyed by property (`mouse_bite.drill` for record fields).
    pub origins: IndexMap<String, String>,
    /// `BOARD.REFERENCE` components fitted after depanelization: their body
    /// overhangs may reach rails or neighbours on the panel.
    pub assemble_after_depanel: Vec<String>,
    pub boards: Vec<PanelBoard>,
    pub tabs: Vec<PanelTab>,
}

/// Panel values without a preset.
const BUILTIN: Defaults = Defaults {
    spacing: 2_000_000,
    rails: Rails { top: 0, bottom: 0, left: 0, right: 0 },
    tab_width: 3_000_000,
    mouse_bite: MouseBite { drill: 500_000, pitch: 750_000, inset: 250_000 },
    fiducials: 0,
    fiducial_offset: None,
    tooling_hole: None,
};

fn length(value: &Value, location: &Location, positive: bool) -> Result<i64> {
    let Value::Scalar(Scalar::Quantity { value, unit }) = value else {
        return fail("expected a length quantity (mm, um or m)", location);
    };
    if !Dimension::Length.units().iter().any(|(name, _)| name == unit) {
        return fail("expected a length quantity (mm, um or m)", location);
    }
    let invalid = |message: String| CopperScriptError::new("PNL001", message, location.clone());
    let number = Decimal::parse(value).map_err(|e| invalid(e.to_string()))?;
    let quantity = Quantity::of(Dimension::Length, &number, unit).map_err(|e| invalid(e.to_string()))?;
    let metres = quantity.in_unit("m").map_err(|e| invalid(e.to_string()))?;
    let nm = metres.mul(&Decimal::from_i64(1_000_000_000)).map_err(|e| invalid(e.to_string()))?;
    if nm != nm.to_integral_value(Rounding::HalfEven) {
        return fail("length must be an exact integer number of nanometres", location);
    }
    if if positive { nm <= Decimal::zero() } else { nm < Decimal::zero() } {
        return fail(if positive { "length must be positive" } else { "length must not be negative" }, location);
    }
    nm.to_i64().ok_or_else(|| invalid("length out of range".to_owned()))
}

fn text<'v>(value: &'v Value, location: &Location) -> Result<&'v str> {
    match value {
        Value::Scalar(Scalar::Str(text)) if !text.is_empty() => Ok(text),
        _ => fail("expected a nonempty name or string", location),
    }
}

fn point(value: &Value, location: &Location) -> Result<[i64; 2]> {
    match value {
        Value::List(items) if items.len() == 2 => {
            Ok([length(&items[0], location, false)?, length(&items[1], location, false)?])
        }
        _ => fail("a point requires exactly two lengths", location),
    }
}

fn quarter_turns(value: &Value, location: &Location) -> Result<u8> {
    let Value::Scalar(Scalar::Quantity { value, unit }) = value else {
        return fail("expected an angle quantity in deg", location);
    };
    if unit != "deg" {
        return fail("expected an angle quantity in deg", location);
    }
    let degrees =
        Decimal::parse(value).map_err(|e| CopperScriptError::new("PNL001", e.to_string(), location.clone()))?;
    let turns = degrees.div(&Decimal::from_i64(90)).ok();
    match turns.filter(|t| *t == t.to_integral_value(Rounding::HalfEven)).and_then(|t| t.to_i64()) {
        Some(turns) => Ok(turns.rem_euclid(4) as u8),
        None => fail("board rotation must be a multiple of 90 deg", location),
    }
}

/// Unknown or duplicate keys of a property map.
fn check_keys(map: &IndexMap<String, Value>, allowed: &[&str], what: &str, location: &Location) -> Result<()> {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return fail(format!("unknown {what} property {key:?}; expected one of {}", allowed.join(", ")), location);
        }
    }
    Ok(())
}

fn required<'v>(map: &'v IndexMap<String, Value>, key: &str, what: &str, location: &Location) -> Result<&'v Value> {
    map.get(key).ok_or_else(|| CopperScriptError::new("PNL001", format!("{what} requires {key}"), location.clone()))
}

fn board(item: &MacroItemDecl) -> Result<PanelBoard> {
    let location = &item.location;
    // The panel prefixes nets `NAME/` and references `NAME-`, and board rules
    // match those prefixes case-insensitively: a name with `-` or `/` could
    // prefix another board's names.
    if item.name.is_empty() || !item.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return fail(format!("board name {:?} must use letters, digits and underscores only", item.name), location);
    }
    // The report names a tab's far end "rail"; the frame's mouse bites are
    // footprints TAB-NAME, which board TAB's references would look like.
    if item.name.eq_ignore_ascii_case("rail") || item.name.eq_ignore_ascii_case("tab") {
        return fail(format!("{:?} is reserved for the panel's frame; name the board otherwise", item.name), location);
    }
    check_keys(&item.parameters, &["source", "pcb", "at", "rotation"], "board", location)?;
    let what = format!("board {}", item.name);
    Ok(PanelBoard {
        name: item.name.clone(),
        location: location.clone(),
        source: text(required(&item.parameters, "source", &what, location)?, location)?.to_owned(),
        pcb: text(required(&item.parameters, "pcb", &what, location)?, location)?.to_owned(),
        at: point(required(&item.parameters, "at", &what, location)?, location)?,
        quarter_turns: match item.parameters.get("rotation") {
            Some(value) => quarter_turns(value, location)?,
            None => 0,
        },
    })
}

fn tab(item: &MacroItemDecl, default_width: i64) -> Result<PanelTab> {
    let location = &item.location;
    check_keys(&item.parameters, &["board", "side", "edge", "at", "width"], "tab", location)?;
    let what = format!("tab {}", item.name);
    let edge = match (item.parameters.get("side"), item.parameters.get("edge")) {
        (Some(side), None) => {
            let name = text(side, location)?;
            TabEdge::Side(Side::parse(name).ok_or_else(|| {
                CopperScriptError::new(
                    "PNL001",
                    format!("unknown side {name:?}; expected left, right, top or bottom"),
                    location.clone(),
                )
            })?)
        }
        (None, Some(edge)) => TabEdge::Named(text(edge, location)?.to_owned()),
        _ => return fail(format!("{what} requires exactly one of side or edge"), location),
    };
    Ok(PanelTab {
        name: item.name.clone(),
        location: location.clone(),
        board: text(required(&item.parameters, "board", &what, location)?, location)?.to_owned(),
        edge,
        at: length(required(&item.parameters, "at", &what, location)?, location, false)?,
        width: match item.parameters.get("width") {
            Some(value) => length(value, location, true)?,
            None => default_width,
        },
    })
}

/// `lower_panel`: a parsed `panel` document as a checked specification.
pub fn lower_panel(document: &Document) -> Result<PanelSpec> {
    if document.kind != DocumentKind::Panel {
        return fail(format!("expected a panel document, found {}", document.kind.as_str()), &document.location);
    }
    let mut properties: IndexMap<String, (&Value, &Location)> = IndexMap::new();
    let mut items: Vec<&MacroItemDecl> = Vec::new();
    for declaration in &document.declarations {
        match declaration {
            Declaration::MacroProperty(property) => {
                if properties.insert(property.name.clone(), (&property.value, &property.location)).is_some() {
                    return fail(format!("duplicate panel property {}", property.name), &property.location);
                }
            }
            Declaration::MacroItem(item) if item.kind == "board" || item.kind == "tab" => items.push(item),
            Declaration::MacroItem(item) => {
                return fail(
                    format!("unknown panel declaration {:?}; expected board or tab", item.kind),
                    &item.location,
                );
            }
            _ => return fail("panels hold properties, board blocks and tab blocks only", &document.location),
        }
    }
    let allowed = [
        "preset",
        "spacing",
        "rails",
        "tab_width",
        "mouse_bite",
        "fiducials",
        "fiducial_offset",
        "tooling_hole",
        "assemble_after_depanel",
    ];
    for (name, (_, location)) in &properties {
        if !allowed.contains(&name.as_str()) {
            return fail(format!("unknown panel property {name:?}; expected one of {}", allowed.join(", ")), location);
        }
    }
    let get = |name: &str| properties.get(name).copied();
    let preset = match get("preset") {
        Some((value, location)) => {
            let name = text(value, location)?;
            Some(panel_presets::preset(name).ok_or_else(|| {
                CopperScriptError::new(
                    "PNL001",
                    format!("unknown panel preset {name:?}; expected one of {}", panel_presets::names().join(", ")),
                    location.clone(),
                )
            })?)
        }
        None => None,
    };
    let base = preset.map_or(BUILTIN, |p| p.defaults);
    let fallback = preset.map_or_else(|| "default".to_owned(), |p| format!("preset {}", p.name));
    let mut origins: IndexMap<String, String> = IndexMap::new();
    let mut origin = |key: &str, explicit: bool| {
        origins.insert(key.to_owned(), if explicit { "explicit".to_owned() } else { fallback.clone() });
    };
    let spacing = match get("spacing") {
        Some((value, location)) => length(value, location, true)?,
        None => base.spacing,
    };
    origin("spacing", get("spacing").is_some());
    let tab_width = match get("tab_width") {
        Some((value, location)) => length(value, location, true)?,
        None => base.tab_width,
    };
    origin("tab_width", get("tab_width").is_some());
    let mut rails = base.rails;
    if let Some((value, location)) = get("rails") {
        let Value::Map(map) = value else { return fail("rails is a record { top = ...; bottom = ...; }", location) };
        check_keys(map, &["top", "bottom", "left", "right"], "rails", location)?;
        let side = |key: &str| map.get(key).map_or(Ok(0), |v| length(v, location, false));
        rails = Rails { top: side("top")?, bottom: side("bottom")?, left: side("left")?, right: side("right")? };
    }
    origin("rails", get("rails").is_some());
    let mut mouse_bite = base.mouse_bite;
    let mut given: Vec<&str> = Vec::new();
    if let Some((value, location)) = get("mouse_bite") {
        let Value::Map(map) = value else {
            return fail("mouse_bite is a record { drill = ...; pitch = ...; }", location);
        };
        check_keys(map, &["drill", "pitch", "inset"], "mouse_bite", location)?;
        if let Some(v) = map.get("drill") {
            mouse_bite.drill = length(v, location, true)?;
        }
        if let Some(v) = map.get("pitch") {
            mouse_bite.pitch = length(v, location, true)?;
        }
        if let Some(v) = map.get("inset") {
            mouse_bite.inset = length(v, location, false)?;
        }
        given = map.keys().map(String::as_str).collect();
        if mouse_bite.pitch <= mouse_bite.drill {
            return fail("mouse_bite pitch must exceed its drill, or the holes merge into a slot", location);
        }
    }
    for field in ["drill", "pitch", "inset"] {
        origin(&format!("mouse_bite.{field}"), given.contains(&field));
    }
    let fiducials = match get("fiducials") {
        Some((Value::Scalar(Scalar::Int(number)), location)) => match number.to_u32() {
            Some(count) if count <= 4 => count,
            _ => return fail("fiducials is a count from 0 to 4", location),
        },
        Some((_, location)) => return fail("fiducials is a count from 0 to 4", location),
        None => base.fiducials,
    };
    origin("fiducials", get("fiducials").is_some());
    let fiducial_offset = match get("fiducial_offset") {
        Some((value, location)) => Some(length(value, location, true)?),
        None => base.fiducial_offset,
    };
    origin("fiducial_offset", get("fiducial_offset").is_some());
    let tooling_hole = match get("tooling_hole") {
        Some((value, location)) => Some(length(value, location, true)?),
        None => base.tooling_hole,
    };
    origin("tooling_hole", get("tooling_hole").is_some());
    let assemble_after_depanel = match get("assemble_after_depanel") {
        Some((Value::List(values), location)) => {
            values.iter().map(|v| text(v, location).map(str::to_owned)).collect::<Result<Vec<_>>>()?
        }
        Some((_, location)) => return fail("assemble_after_depanel is a list of \"BOARD.REFERENCE\" names", location),
        None => Vec::new(),
    };
    let mut boards: Vec<PanelBoard> = Vec::new();
    let mut tabs: Vec<PanelTab> = Vec::new();
    for item in items {
        if item.kind == "board" {
            if boards.iter().any(|b| b.name.eq_ignore_ascii_case(&item.name)) {
                return fail(
                    format!("duplicate board {} (board names are compared ignoring case)", item.name),
                    &item.location,
                );
            }
            boards.push(board(item)?);
        } else {
            if tabs.iter().any(|t| t.name == item.name) {
                return fail(format!("duplicate tab {}", item.name), &item.location);
            }
            tabs.push(tab(item, tab_width)?);
        }
    }
    if boards.is_empty() {
        return fail("a panel needs at least one board", &document.location);
    }
    let names: IndexSet<&str> = boards.iter().map(|b| b.name.as_str()).collect();
    for tab in &tabs {
        if !names.contains(tab.board.as_str()) {
            return fail(format!("tab {} names unknown board {}", tab.name, tab.board), &tab.location);
        }
    }
    for entry in &assemble_after_depanel {
        let board = entry.split_once('.').map(|(board, _)| board);
        if !board.is_some_and(|b| names.contains(b)) {
            return fail(
                format!("assemble_after_depanel entry {entry:?} is not BOARD.REFERENCE of a panel board"),
                &document.location,
            );
        }
    }
    Ok(PanelSpec {
        name: document.name.clone(),
        location: document.location.clone(),
        spacing,
        rails,
        tab_width,
        mouse_bite,
        fiducials,
        tooling_hole,
        fiducial_offset,
        preset: preset.map(|p| p.name.to_owned()),
        origins,
        assemble_after_depanel,
        boards,
        tabs,
    })
}

/// Parse and lower a `panel` file.
pub fn load_panel(path: &Path) -> Result<PanelSpec> {
    let display = path.to_string_lossy();
    let source = std::fs::read_to_string(path).map_err(|e| {
        CopperScriptError::new(
            "PNL002",
            format!("cannot read {display}: {e}"),
            Location { filename: display.as_ref().into(), offset: 0, line: 1, column: 1 },
        )
    })?;
    let document = crate::parser::parse(&source, &display)?;
    lower_panel(&document)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowered(source: &str) -> Result<PanelSpec> {
        lower_panel(&crate::parser::parse(source, "panel.copper").expect("a parsed panel"))
    }

    const PANEL: &str = r#"panel Demo {
        spacing = 2 mm;
        rails = { top = 5 mm; bottom = 5 mm; };
        mouse_bite = { drill = 0.5 mm; pitch = 0.8 mm; };
        fiducials = 3;
        tooling_hole = 1.152 mm;
        board A1 { source = "a1.copper"; pcb = "build/a1.kicad_pcb"; at = (5 mm, 7 mm); }
        board A2 { source = "a2.copper"; pcb = "build/a2.kicad_pcb"; at = (47 mm, 7 mm); rotation = 270 deg; }
        tab a1_right { board = A1; side = right; at = 10 mm; width = 4 mm; }
        tab a2_left { board = A2; edge = "LEFT"; at = 12.5 mm; }
    }"#;

    #[test]
    fn a_panel_lowers_with_units_defaults_and_references() {
        let spec = lowered(PANEL).expect("a valid panel");
        assert_eq!((spec.name.as_str(), spec.spacing, spec.tab_width), ("Demo", 2_000_000, 3_000_000));
        assert_eq!(spec.rails, Rails { top: 5_000_000, bottom: 5_000_000, left: 0, right: 0 });
        assert_eq!(spec.mouse_bite, MouseBite { drill: 500_000, pitch: 800_000, inset: 250_000 });
        assert_eq!((spec.fiducials, spec.tooling_hole), (3, Some(1_152_000)));
        assert_eq!(spec.boards[1].at, [47_000_000, 7_000_000]);
        assert_eq!(spec.boards[1].quarter_turns, 3);
        assert_eq!(spec.tabs[0].edge, TabEdge::Side(Side::Right));
        assert_eq!((spec.tabs[0].at, spec.tabs[0].width), (10_000_000, 4_000_000));
        assert_eq!(spec.tabs[1].edge, TabEdge::Named("LEFT".to_owned()));
        assert_eq!(spec.tabs[1].width, 3_000_000);
    }

    #[test]
    fn panel_mistakes_fail_at_their_declaration() {
        for (edit, message) in [
            (("board = A1; side = right;", "board = A9; side = right;"), "unknown board A9"),
            (("rotation = 270 deg;", "rotation = 45 deg;"), "multiple of 90 deg"),
            (("pitch = 0.8 mm;", "pitch = 0.4 mm;"), "pitch must exceed its drill"),
            (("side = right;", "side = right; edge = \"R\";"), "exactly one of side or edge"),
            (("fiducials = 3;", "fiducials = 7;"), "count from 0 to 4"),
            (("spacing = 2 mm;", "spacing = 2 mm; spacing_x = 1 mm;"), "unknown panel property"),
            (("board A2 {", "board a1 {"), "duplicate board a1"),
            (("board A2 {", "board A-B {"), "letters, digits and underscores only"),
            (("board A2 {", "board Rail {"), "reserved for the panel's frame"),
            (("board A2 {", "board TAB {"), "reserved for the panel's frame"),
            (
                ("spacing = 2 mm;", "spacing = 2 mm; preset = \"acme\";"),
                "unknown panel preset \"acme\"; expected one of jlcpcb",
            ),
        ] {
            let source = PANEL.replacen(edit.0, edit.1, 1);
            assert_ne!(source, PANEL, "{edit:?}");
            let error = lowered(&source).expect_err(message).to_string();
            assert!(error.contains(message), "{error}");
        }
    }

    #[test]
    fn a_preset_fills_what_the_panel_leaves_out_and_records_origins() {
        let source = r#"panel Fab {
            preset = "jlcpcb";
            tab_width = 6 mm;
            mouse_bite = { inset = 0.3 mm; };
            board A1 { source = "a1.copper"; pcb = "a1.kicad_pcb"; at = (0 mm, 0 mm); }
        }"#;
        let spec = lowered(source).expect("a valid panel");
        assert_eq!(spec.preset.as_deref(), Some("jlcpcb"));
        assert_eq!((spec.spacing, spec.tab_width), (2_000_000, 6_000_000));
        assert_eq!(spec.rails, Rails { top: 5_000_000, bottom: 5_000_000, left: 0, right: 0 });
        assert_eq!(spec.mouse_bite, MouseBite { drill: 600_000, pitch: 950_000, inset: 300_000 });
        assert_eq!((spec.fiducials, spec.fiducial_offset, spec.tooling_hole), (3, Some(3_850_000), Some(2_000_000)));
        assert_eq!(spec.origins["tab_width"], "explicit");
        assert_eq!(spec.origins["spacing"], "preset jlcpcb");
        assert_eq!(spec.origins["mouse_bite.inset"], "explicit");
        assert_eq!(spec.origins["mouse_bite.drill"], "preset jlcpcb");
        // Without a preset the built-in values apply.
        let plain = lowered(&source.replace("preset = \"jlcpcb\";", "")).expect("a valid panel");
        assert_eq!((plain.preset, plain.tab_width, plain.fiducials, plain.tooling_hole), (None, 6_000_000, 0, None));
        assert_eq!(plain.origins["spacing"], "default");
        // A merged mouse bite still needs its pitch above its drill.
        let error = lowered(&source.replace("inset = 0.3 mm;", "pitch = 0.5 mm;")).expect_err("merged").to_string();
        assert!(error.contains("pitch must exceed its drill"), "{error}");
    }
}

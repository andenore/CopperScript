//! Fab presets for `panel` documents: default panel values and the rules a
//! fab documents for customer panels, each with the page it comes from.
//! `preset = "jlcpcb";` fills every property the panel leaves out, and
//! `copper panelize` checks the composed panel against the preset's rules
//! (see docs/panelization.md). Values were read from the fabs' pages on
//! 2026-11-07; where a fab's pages disagree, the rule's note says so.

use crate::panel::{MouseBite, Rails};

const JLC_CAPABILITIES: &str = "https://jlcpcb.com/capabilities/pcb-capabilities";
const JLC_PANEL_BLOG: &str = "https://jlcpcb.com/blog/pcb-panel-size";
const JLC_PANELIZATION: &str = "https://jlcpcb.com/help/article/pcb-panelization";
const JLC_ASSEMBLY: &str = "https://jlcpcb.com/capabilities/pcb-assembly-capabilities";
const JLC_EXTRA_CHARGES: &str = "https://jlcpcb.com/help/article/in-what-cases-will-there-be-charged-extra";
const PCBWAY_ASSEMBLY: &str = "https://www.pcbway.com/pcb_prototype/Panel_Requirements_for_Assembly.html";
const PCBWAY_HELP: &str = "https://pcbway.com/helpcenter/design_instruction/PCB_Panelization__Breakaway_Rails__Fiducial_Marks__Tooling_Holes.html";
const EC_PANEL_BASICS: &str =
    "https://www.eurocircuits.com/technical-guidelines/panel-guidelines/customer-panel-basics/";

/// How a broken rule is reported: `Fail` is outside a documented limit,
/// `Warn` a recommendation or a cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Fail,
    Warn,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Fail => "fail",
            Level::Warn => "warn",
        }
    }
}

/// What a rule checks. Lengths are nanometres; sizes are (short, long) sides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    /// Milled gap between boards, and between boards and rails, at least this.
    MinSpacing(i64),
    /// Every rail present at least this wide.
    MinRail(i64),
    /// Rails on these sides (top, bottom, left, right) are required.
    RailsRequired {
        top: bool,
        bottom: bool,
        left: bool,
        right: bool,
    },
    /// Distance from every board to the panel's outer edge at least this.
    MinBoardToEdge(i64),
    MinTabWidth(i64),
    MinDrill(i64),
    MaxDrill(i64),
    /// Material between neighbouring mouse-bite holes (pitch - drill).
    MinWeb(i64),
    /// Mouse-bite holes along each tab end.
    MinHolesPerTab(i64),
    /// Tooling holes present with this diameter.
    ToolingHole(i64),
    /// At least this many fiducials.
    MinFiducials(i64),
    /// Fiducial centres this far from the panel's outer edge.
    FiducialOffset(i64),
    /// Both panel sides at least (short, long).
    MinPanelSize(i64, i64),
    /// Panel sides at most (short, long).
    MaxPanelSize(i64, i64),
    /// Distinct board designs (routed boards) on one panel at most.
    MaxDesigns(i64),
    /// Distinct designs beyond this may be charged.
    FreeDesigns(i64),
}

impl RuleKind {
    pub fn name(self) -> &'static str {
        match self {
            RuleKind::MinSpacing(_) => "min_spacing",
            RuleKind::MinRail(_) => "min_rail",
            RuleKind::RailsRequired { .. } => "rails_required",
            RuleKind::MinBoardToEdge(_) => "min_board_to_edge",
            RuleKind::MinTabWidth(_) => "min_tab_width",
            RuleKind::MinDrill(_) => "min_mouse_bite_drill",
            RuleKind::MaxDrill(_) => "max_mouse_bite_drill",
            RuleKind::MinWeb(_) => "min_mouse_bite_web",
            RuleKind::MinHolesPerTab(_) => "min_mouse_bites_per_tab",
            RuleKind::ToolingHole(_) => "tooling_hole",
            RuleKind::MinFiducials(_) => "min_fiducials",
            RuleKind::FiducialOffset(_) => "fiducial_offset",
            RuleKind::MinPanelSize(..) => "min_panel_size",
            RuleKind::MaxPanelSize(..) => "max_panel_size",
            RuleKind::MaxDesigns(_) => "max_designs",
            RuleKind::FreeDesigns(_) => "free_designs",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    pub level: Level,
    pub source: &'static str,
    /// Context for the reader: a conflict between the fab's pages, or scope.
    pub note: &'static str,
}

/// Values a preset gives the properties a panel leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Defaults {
    pub spacing: i64,
    pub rails: Rails,
    pub tab_width: i64,
    pub mouse_bite: MouseBite,
    pub fiducials: u32,
    pub fiducial_offset: Option<i64>,
    pub tooling_hole: Option<i64>,
    /// The router bit's diameter: inside corners and slot ends come out
    /// rounded to its radius.
    pub cutter: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub summary: &'static str,
    pub defaults: Defaults,
    pub rules: &'static [Rule],
}

const fn rule(kind: RuleKind, level: Level, source: &'static str, note: &'static str) -> Rule {
    Rule { kind, level, source, note }
}

const MM: i64 = 1_000_000;
const UM: i64 = 1_000;

const JLC_DEFAULTS: Defaults = Defaults {
    spacing: 2 * MM,
    rails: Rails { top: 5 * MM, bottom: 5 * MM, left: 0, right: 0 },
    tab_width: 5 * MM,
    mouse_bite: MouseBite { drill: 600 * UM, pitch: 950 * UM, inset: 250 * UM },
    fiducials: 3,
    fiducial_offset: Some(3_850 * UM),
    tooling_hole: Some(2 * MM),
    cutter: 2 * MM,
};

const JLC_FAB_RULES: [Rule; 11] = [
    rule(
        RuleKind::MinSpacing(1_600 * UM),
        Level::Fail,
        JLC_CAPABILITIES,
        "mouse-bite panels: 1.6 or 2 mm between boards",
    ),
    rule(RuleKind::MinRail(3 * MM), Level::Fail, JLC_CAPABILITIES, "minimum tooling edge; 5 mm for SMT assembly"),
    rule(
        RuleKind::MinTabWidth(5 * MM),
        Level::Fail,
        JLC_CAPABILITIES,
        "breakaway tab at least 4 mm, 5 mm with mouse bites",
    ),
    rule(RuleKind::MinDrill(500 * UM), Level::Fail, JLC_CAPABILITIES, "mouse-bite holes 0.5-0.8 mm"),
    rule(RuleKind::MaxDrill(800 * UM), Level::Fail, JLC_CAPABILITIES, "mouse-bite holes 0.5-0.8 mm"),
    rule(
        RuleKind::MinWeb(300 * UM),
        Level::Fail,
        JLC_PANEL_BLOG,
        "0.35-0.4 mm recommended, 0.3 mm floor; the capabilities page lists 0.2-0.3 mm",
    ),
    rule(RuleKind::MinHolesPerTab(5), Level::Warn, JLC_PANEL_BLOG, "0.6 mm holes in sets of 5-8"),
    rule(RuleKind::ToolingHole(2 * MM), Level::Warn, JLC_CAPABILITIES, "2 mm non-plated tooling holes on 5 mm rails"),
    rule(
        RuleKind::FiducialOffset(3_850 * UM),
        Level::Warn,
        JLC_CAPABILITIES,
        "1 mm fiducials centred 3.85 mm from the panel edge",
    ),
    rule(
        RuleKind::MaxDesigns(10),
        Level::Fail,
        JLC_EXTRA_CHARGES,
        "a customer panel holds at most 10 different designs",
    ),
    rule(
        RuleKind::FreeDesigns(5),
        Level::Warn,
        JLC_PANELIZATION,
        "designs past the fifth are charged; the extra-charges page says past the first",
    ),
];

const JLC_SMT_RULES: [Rule; 15] = [
    JLC_FAB_RULES[0],
    rule(RuleKind::MinRail(5 * MM), Level::Fail, JLC_CAPABILITIES, "5 mm tooling edges for SMT assembly"),
    rule(
        RuleKind::RailsRequired { top: true, bottom: true, left: false, right: false },
        Level::Fail,
        JLC_ASSEMBLY,
        "Standard PCBA panels need edge rails",
    ),
    JLC_FAB_RULES[2],
    JLC_FAB_RULES[3],
    JLC_FAB_RULES[4],
    JLC_FAB_RULES[5],
    JLC_FAB_RULES[6],
    rule(RuleKind::ToolingHole(2 * MM), Level::Fail, JLC_CAPABILITIES, "2 mm non-plated tooling holes on 5 mm rails"),
    rule(RuleKind::MinFiducials(3), Level::Fail, JLC_ASSEMBLY, "Standard PCBA panels need fiducials"),
    JLC_FAB_RULES[8],
    rule(
        RuleKind::MinPanelSize(70 * MM, 70 * MM),
        Level::Fail,
        JLC_ASSEMBLY,
        "Standard PCBA panels 70 x 70 to 250 x 250 mm",
    ),
    rule(
        RuleKind::MaxPanelSize(250 * MM, 250 * MM),
        Level::Warn,
        JLC_ASSEMBLY,
        "250 x 250 mm is the recommended maximum; larger panels carry more risk",
    ),
    JLC_FAB_RULES[9],
    JLC_FAB_RULES[10],
];

const PCBWAY_RULES: [Rule; 9] = [
    rule(RuleKind::MinSpacing(1_600 * UM), Level::Fail, PCBWAY_ASSEMBLY, "tab-routed boards 1.6 mm apart"),
    rule(
        RuleKind::MinRail(3 * MM),
        Level::Fail,
        PCBWAY_HELP,
        "breakaway rails at least 3 mm; 5, 6, 7, 8 or 10 mm standard",
    ),
    rule(RuleKind::MinTabWidth(2 * MM), Level::Fail, PCBWAY_ASSEMBLY, "solid tabs at least 2 mm wide"),
    rule(
        RuleKind::MinDrill(450 * UM),
        Level::Fail,
        PCBWAY_ASSEMBLY,
        "stamp holes at least 0.45 mm, 0.55-0.6 mm regular",
    ),
    rule(RuleKind::MinWeb(350 * UM), Level::Fail, PCBWAY_ASSEMBLY, "stamp holes at least 0.35 mm apart"),
    rule(RuleKind::MinHolesPerTab(5), Level::Warn, PCBWAY_ASSEMBLY, "usually 5-6 stamp holes per tab"),
    rule(RuleKind::ToolingHole(2 * MM), Level::Warn, PCBWAY_HELP, "tooling holes generally 2 mm, non-plated"),
    rule(
        RuleKind::MinPanelSize(50 * MM, 50 * MM),
        Level::Warn,
        PCBWAY_ASSEMBLY,
        "assembly panels 50 x 50 to 330 x 530 mm",
    ),
    rule(
        RuleKind::MaxPanelSize(330 * MM, 530 * MM),
        Level::Warn,
        PCBWAY_ASSEMBLY,
        "assembly panels 50 x 50 to 330 x 530 mm",
    ),
];

const EUROCIRCUITS_RULES: [Rule; 6] = [
    rule(RuleKind::MinSpacing(2 * MM), Level::Fail, EC_PANEL_BASICS, "break routing with a 2.00 mm router"),
    rule(RuleKind::MinRail(5 * MM), Level::Warn, EC_PANEL_BASICS, "recommended border at least 5 mm on all four sides"),
    rule(
        RuleKind::RailsRequired { top: true, bottom: true, left: true, right: true },
        Level::Warn,
        EC_PANEL_BASICS,
        "recommended border on all four sides",
    ),
    rule(
        RuleKind::MinBoardToEdge(7 * MM),
        Level::Fail,
        EC_PANEL_BASICS,
        "break routing needs 7.00 mm from any board to the panel edge",
    ),
    rule(
        RuleKind::MinPanelSize(36 * MM, 46 * MM),
        Level::Warn,
        EC_PANEL_BASICS,
        "Eurocircuits assembly panels 36 x 46 to 326 x 426 mm",
    ),
    rule(
        RuleKind::MaxPanelSize(326 * MM, 426 * MM),
        Level::Warn,
        EC_PANEL_BASICS,
        "Eurocircuits assembly panels 36 x 46 to 326 x 426 mm",
    ),
];

pub const PRESETS: [Preset; 4] = [
    Preset {
        name: "jlcpcb",
        summary: "JLCPCB customer panel with mouse-bite tabs (fabrication)",
        defaults: JLC_DEFAULTS,
        rules: &JLC_FAB_RULES,
    },
    Preset {
        name: "jlcpcb-smt",
        summary: "JLCPCB customer panel for Standard PCBA assembly",
        defaults: JLC_DEFAULTS,
        rules: &JLC_SMT_RULES,
    },
    Preset {
        name: "pcbway",
        summary: "PCBWay tab-routed panel",
        defaults: Defaults {
            spacing: 1_600 * UM,
            rails: Rails { top: 5 * MM, bottom: 5 * MM, left: 0, right: 0 },
            tab_width: 5 * MM,
            mouse_bite: MouseBite { drill: 600 * UM, pitch: 950 * UM, inset: 250 * UM },
            fiducials: 3,
            fiducial_offset: None,
            tooling_hole: Some(2 * MM),
            cutter: 1_600 * UM,
        },
        rules: &PCBWAY_RULES,
    },
    Preset {
        name: "eurocircuits",
        summary: "Eurocircuits break-routed customer panel (Eurocircuits places the tabs and adds fiducials and tooling holes)",
        defaults: Defaults {
            spacing: 2 * MM,
            rails: Rails { top: 5 * MM, bottom: 5 * MM, left: 5 * MM, right: 5 * MM },
            tab_width: 5 * MM,
            mouse_bite: MouseBite { drill: 600 * UM, pitch: 950 * UM, inset: 250 * UM },
            fiducials: 0,
            fiducial_offset: None,
            tooling_hole: None,
            cutter: 2 * MM,
        },
        rules: &EUROCIRCUITS_RULES,
    },
];

/// A preset by name.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Preset names, for messages.
pub fn names() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.name).collect()
}

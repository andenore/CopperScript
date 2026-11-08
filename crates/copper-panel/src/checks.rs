//! Fab checks: the composed panel against its preset's documented rules,
//! each result with the value found, the limit and the page it comes from;
//! and the panel's effective configuration with each value's origin.

use copper_json::Value as Json;
use copper_lang::panel::PanelSpec;
use copper_lang::panel_presets::{self, Level, Rule, RuleKind};
use indexmap::IndexSet;

use crate::compose::mm;
use crate::frame::{BuiltTab, fiducial_offsets, holes_per_tab};

/// Fiducial offsets within this of the documented value pass.
const OFFSET_TOLERANCE: i64 = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub rule: &'static str,
    pub status: Status,
    /// What the panel has, and what the rule asks for.
    pub value: String,
    pub limit: String,
    pub source: &'static str,
    pub note: &'static str,
}

fn length(nm: i64) -> String {
    format!("{} mm", mm(nm))
}

fn size(a: i64, b: i64) -> String {
    format!("{} x {} mm", mm(a), mm(b))
}

/// Distinct routed boards on the panel: identical boards count once.
pub fn designs(spec: &PanelSpec) -> usize {
    spec.boards.iter().map(|b| b.pcb.replace('\\', "/")).collect::<IndexSet<_>>().len()
}

/// The smallest of `values` with the names that have it, or `None`.
fn smallest<'a>(values: impl Iterator<Item = (i64, &'a str)>) -> Option<(i64, Vec<&'a str>)> {
    let mut best: Option<(i64, Vec<&str>)> = None;
    for (value, name) in values {
        match &mut best {
            Some((least, names)) if value == *least => names.push(name),
            Some((least, _)) if value > *least => {}
            _ => best = Some((value, vec![name])),
        }
    }
    best
}

fn named(names: &[&str]) -> String {
    match names {
        [one] => format!(" (tab {one})"),
        [first, ..] => format!(" (tab {first} and {} more)", names.len() - 1),
        [] => String::new(),
    }
}

/// Check one rule; `bounds` is the panel outline `(x0, y0, x1, y1)`.
fn check(rule: &Rule, spec: &PanelSpec, bounds: (i64, i64, i64, i64), tabs: &[BuiltTab], gap: i64) -> Check {
    let rails = spec.rails;
    let present: Vec<(&str, i64)> =
        [("top", rails.top), ("bottom", rails.bottom), ("left", rails.left), ("right", rails.right)]
            .into_iter()
            .filter(|(_, w)| *w > 0)
            .collect();
    let (w, h) = (bounds.2 - bounds.0, bounds.3 - bounds.1);
    let (short, long) = (w.min(h), w.max(h));
    let (ok, value, limit) = match rule.kind {
        RuleKind::MinSpacing(v) => {
            let gap = gap.min(spec.spacing);
            (gap >= v, length(gap), format!(">= {}", length(v)))
        }
        RuleKind::MinRail(v) => match present.iter().min_by_key(|(_, w)| *w) {
            Some((side, width)) => (*width >= v, format!("{} ({side})", length(*width)), format!(">= {}", length(v))),
            None => (true, "no rails".to_owned(), format!(">= {}", length(v))),
        },
        RuleKind::RailsRequired { top, bottom, left, right } => {
            let wanted: Vec<&str> = [("top", top), ("bottom", bottom), ("left", left), ("right", right)]
                .into_iter()
                .filter(|(_, want)| *want)
                .map(|(side, _)| side)
                .collect();
            let missing: Vec<&str> =
                wanted.iter().copied().filter(|side| !present.iter().any(|(s, _)| s == side)).collect();
            let have: Vec<&str> = present.iter().map(|(s, _)| *s).collect();
            (missing.is_empty(), if have.is_empty() { "none".to_owned() } else { have.join(", ") }, wanted.join(", "))
        }
        RuleKind::MinBoardToEdge(v) => {
            let reach = |rail: i64| if rail > 0 { spec.spacing + rail } else { 0 };
            let sides = [
                (reach(rails.top), "top"),
                (reach(rails.bottom), "bottom"),
                (reach(rails.left), "left"),
                (reach(rails.right), "right"),
            ];
            let (least, side) = sides.into_iter().min_by_key(|(d, _)| *d).unwrap_or((0, "top"));
            (least >= v, format!("{} ({side})", length(least)), format!(">= {}", length(v)))
        }
        RuleKind::MinTabWidth(v) => match smallest(tabs.iter().map(|t| (t.width, t.name.as_str()))) {
            Some((width, names)) => {
                (width >= v, format!("{}{}", length(width), named(&names)), format!(">= {}", length(v)))
            }
            None => (true, "no tabs".to_owned(), format!(">= {}", length(v))),
        },
        RuleKind::MinDrill(v) => {
            (spec.mouse_bite.drill >= v, length(spec.mouse_bite.drill), format!(">= {}", length(v)))
        }
        RuleKind::MaxDrill(v) => {
            (spec.mouse_bite.drill <= v, length(spec.mouse_bite.drill), format!("<= {}", length(v)))
        }
        RuleKind::MinWeb(v) => {
            let web = spec.mouse_bite.pitch - spec.mouse_bite.drill;
            (web >= v, length(web), format!(">= {}", length(v)))
        }
        RuleKind::MinHolesPerTab(v) => {
            match smallest(tabs.iter().map(|t| (holes_per_tab(t.width, &spec.mouse_bite), t.name.as_str()))) {
                Some((holes, names)) => (holes >= v, format!("{holes}{}", named(&names)), format!(">= {v}")),
                None => (true, "no tabs".to_owned(), format!(">= {v}")),
            }
        }
        RuleKind::ToolingHole(v) => match spec.tooling_hole {
            Some(d) => (d == v, length(d), length(v)),
            None => (false, "none".to_owned(), length(v)),
        },
        RuleKind::MinFiducials(v) => (i64::from(spec.fiducials) >= v, spec.fiducials.to_string(), format!(">= {v}")),
        RuleKind::FiducialOffset(v) => {
            if spec.fiducials == 0 {
                (true, "no fiducials".to_owned(), length(v))
            } else {
                let offsets = fiducial_offsets(spec);
                let ok = offsets.iter().all(|o| (o - v).abs() <= OFFSET_TOLERANCE);
                let value = if offsets[0] == offsets[1] {
                    length(offsets[0])
                } else {
                    format!("{} (top), {} (bottom)", length(offsets[0]), length(offsets[1]))
                };
                (ok, value, length(v))
            }
        }
        RuleKind::MinPanelSize(s, l) => (short >= s && long >= l, size(w, h), format!(">= {}", size(s, l))),
        RuleKind::MaxPanelSize(s, l) => (short <= s && long <= l, size(w, h), format!("<= {}", size(s, l))),
        RuleKind::MaxDesigns(v) => {
            let n = designs(spec) as i64;
            (n <= v, n.to_string(), format!("<= {v}"))
        }
        RuleKind::FreeDesigns(v) => {
            let n = designs(spec) as i64;
            (n <= v, n.to_string(), format!("<= {v}"))
        }
    };
    let status = match (ok, rule.level) {
        (true, _) => Status::Pass,
        (false, Level::Warn) => Status::Warn,
        (false, Level::Fail) => Status::Fail,
    };
    Check { rule: rule.kind.name(), status, value, limit, source: rule.source, note: rule.note }
}

/// A rule's limit as text, as the checks report it.
pub fn limit(kind: RuleKind) -> String {
    match kind {
        RuleKind::MinSpacing(v)
        | RuleKind::MinRail(v)
        | RuleKind::MinBoardToEdge(v)
        | RuleKind::MinTabWidth(v)
        | RuleKind::MinDrill(v)
        | RuleKind::MinWeb(v) => format!(">= {}", length(v)),
        RuleKind::MaxDrill(v) => format!("<= {}", length(v)),
        RuleKind::ToolingHole(v) | RuleKind::FiducialOffset(v) => length(v),
        RuleKind::MinHolesPerTab(v) | RuleKind::MinFiducials(v) => format!(">= {v}"),
        RuleKind::MaxDesigns(v) | RuleKind::FreeDesigns(v) => format!("<= {v}"),
        RuleKind::MinPanelSize(a, b) => format!(">= {}", size(a, b)),
        RuleKind::MaxPanelSize(a, b) => format!("<= {}", size(a, b)),
        RuleKind::RailsRequired { top, bottom, left, right } => {
            [("top", top), ("bottom", bottom), ("left", left), ("right", right)]
                .into_iter()
                .filter(|(_, want)| *want)
                .map(|(side, _)| side)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// Every rule of the panel's preset (none without a preset).
/// `gap` is the smallest board-to-board distance on the panel (boards sit
/// `spacing` from the rails by construction).
pub fn run(spec: &PanelSpec, bounds: (i64, i64, i64, i64), tabs: &[BuiltTab], gap: i64) -> Vec<Check> {
    let Some(preset) = spec.preset.as_deref().and_then(panel_presets::preset) else { return Vec::new() };
    preset.rules.iter().map(|rule| check(rule, spec, bounds, tabs, gap)).collect()
}

/// The panel's effective values as `(property, value, origin)`, in source syntax.
pub fn config(spec: &PanelSpec) -> Vec<(String, String, String)> {
    let origin = |key: &str| spec.origins.get(key).cloned().unwrap_or_else(|| "default".to_owned());
    let r = spec.rails;
    let b = spec.mouse_bite;
    let optional = |v: Option<i64>, none: &str| v.map_or_else(|| none.to_owned(), length);
    let mut rows = vec![
        ("spacing".to_owned(), length(spec.spacing), origin("spacing")),
        (
            "rails".to_owned(),
            format!(
                "{{ top = {}; bottom = {}; left = {}; right = {}; }}",
                length(r.top),
                length(r.bottom),
                length(r.left),
                length(r.right)
            ),
            origin("rails"),
        ),
        ("tab_width".to_owned(), length(spec.tab_width), origin("tab_width")),
    ];
    for (field, value) in [("drill", b.drill), ("pitch", b.pitch), ("inset", b.inset)] {
        let key = format!("mouse_bite.{field}");
        rows.push((key.clone(), length(value), origin(&key)));
    }
    rows.push(("fiducials".to_owned(), spec.fiducials.to_string(), origin("fiducials")));
    rows.push(("fiducial_offset".to_owned(), optional(spec.fiducial_offset, "rail centre"), origin("fiducial_offset")));
    rows.push(("tooling_hole".to_owned(), optional(spec.tooling_hole, "none"), origin("tooling_hole")));
    rows
}

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// The report's `config` block: each value with its origin.
pub fn config_json(spec: &PanelSpec) -> Json {
    Json::Object(
        config(spec)
            .into_iter()
            .map(|(key, value, origin)| {
                (key, object(vec![("value", Json::from(value.as_str())), ("origin", Json::from(origin.as_str()))]))
            })
            .collect(),
    )
}

/// The report's `checks` block.
pub fn checks_json(spec: &PanelSpec, checks: &[Check]) -> Json {
    let count = |status: Status| Json::from(checks.iter().filter(|c| c.status == status).count() as i64);
    object(vec![
        ("preset", spec.preset.as_deref().map_or(Json::Null, Json::from)),
        ("designs", Json::from(designs(spec) as i64)),
        ("fail", count(Status::Fail)),
        ("warn", count(Status::Warn)),
        (
            "results",
            Json::Array(
                checks
                    .iter()
                    .map(|c| {
                        object(vec![
                            ("rule", Json::from(c.rule)),
                            ("status", Json::from(c.status.as_str())),
                            ("value", Json::from(c.value.as_str())),
                            ("limit", Json::from(c.limit.as_str())),
                            ("note", Json::from(c.note)),
                            ("source", Json::from(c.source)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

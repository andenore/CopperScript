//! The design rules a zone fill depends on: board minimum, copper-to-edge and
//! hole clearances and the net-class clearances from the `.kicad_pro`, and the
//! `clearance`, `hole_clearance` and `edge_clearance` custom rules of a
//! `.kicad_dru`.
//!
//! Custom rule conditions are understood for the subset CopperScript writes:
//! `A.`/`B.` `NetName`, `NetClass` and `Reference` compared with `==` or `!=`
//! against quoted strings (with `*`/`?` wildcards), combined with `&&`, `||`
//! and parentheses, and tried with A and B in both orders. A rule whose
//! constraint changes zone fills in a way not modelled here (physical
//! clearances, thermal relief and zone connection constraints, layer
//! qualifiers, unknown condition syntax) is refused; rules that do not affect
//! fills (courtyards, silkscreen, track widths ...) are ignored.
//!
//! A clearance is the largest applicable value: board minimum, either item's
//! local or net-class clearance, and the last matching custom rule. KiCad lets
//! a matching custom rule replace the net-class value; the two agree whenever
//! rules only raise clearances, as CopperScript's do.

use copper_json::Value;
use copper_physical::kicad_mod::{self, List, Node};
use indexmap::IndexMap;

use crate::board::parse_nm;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clearance,
    HoleClearance,
    EdgeClearance,
}

#[derive(Clone, Debug, PartialEq)]
enum Expression {
    Or(Box<Expression>, Box<Expression>),
    And(Box<Expression>, Box<Expression>),
    Compare { item: char, property: String, value: String, equal: bool },
}

#[derive(Clone, Debug, PartialEq)]
struct Rule {
    kind: Kind,
    min: i64,
    condition: Option<Expression>,
}

/// One side of a rule test: a zone or a board item.
#[derive(Clone, Copy, Debug, Default)]
pub struct Item<'a> {
    pub net: Option<&'a str>,
    pub reference: Option<&'a str>,
    /// A local clearance override (pad, footprint or zone).
    pub local_clearance: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub min_clearance: i64,
    pub edge_clearance: i64,
    pub hole_clearance: i64,
    /// Arc approximation error, nanometres.
    pub max_error: i64,
    /// Board minimums the design-rule check enforces (0 when unset).
    pub min_track_width: i64,
    pub min_via_diameter: i64,
    pub min_via_annular_width: i64,
    pub min_through_hole_diameter: i64,
    pub min_hole_to_hole: i64,
    /// The project's `rule_severities`: violation type to `error`,
    /// `warning` or `ignore`.
    pub severities: IndexMap<String, String>,
    /// Custom-rule constraints neither the filler nor the check models:
    /// rule name and constraint type.
    pub other_constraints: Vec<(String, String)>,
    class_clearance: IndexMap<String, i64>,
    assignments: IndexMap<String, String>,
    patterns: Vec<(String, String)>,
    custom: Vec<Rule>,
}

/// Case-insensitive match of `text` against a pattern with `*` and `?`.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) =
        (pattern.to_lowercase().chars().collect(), text.to_lowercase().chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(position) = star {
            pi = position + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

fn mm(value: Option<&Value>, what: &str) -> Result<Option<i64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Float(f)) => Ok(Some((f * 1e6).round() as i64)),
        Some(Value::Int(i)) => {
            i.to_string().parse::<i64>().map(|v| Some(v * 1_000_000)).map_err(|_| format!("bad {what}"))
        }
        Some(_) => Err(format!("{what} is not a number")),
    }
}

// ---------------------------------------------------------------------------
// condition parser

struct Parser<'a> {
    text: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.text.get(self.index).is_some_and(u8::is_ascii_whitespace) {
            self.index += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        if self.text[self.index..].starts_with(token.as_bytes()) {
            self.index += token.len();
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Expression> {
        let mut left = self.and()?;
        while self.eat("||") {
            left = Expression::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expression> {
        let mut left = self.term()?;
        while self.eat("&&") {
            left = Expression::And(Box::new(left), Box::new(self.term()?));
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expression> {
        if self.eat("(") {
            let inner = self.or()?;
            if !self.eat(")") {
                return Err("unbalanced parenthesis".to_owned());
            }
            return Ok(inner);
        }
        self.skip();
        let item = match self.text.get(self.index) {
            Some(b'A') => 'A',
            Some(b'B') => 'B',
            _ => return Err("expected A. or B.".to_owned()),
        };
        self.index += 1;
        if !self.eat(".") {
            return Err("expected a property".to_owned());
        }
        let start = self.index;
        while self.text.get(self.index).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            self.index += 1;
        }
        let property = String::from_utf8_lossy(&self.text[start..self.index]).into_owned();
        if !matches!(property.as_str(), "NetName" | "NetClass" | "Reference") {
            return Err(format!("property {property} is not supported"));
        }
        let equal = if self.eat("==") {
            true
        } else if self.eat("!=") {
            false
        } else {
            return Err("expected == or !=".to_owned());
        };
        self.skip();
        let quote = *self.text.get(self.index).ok_or("expected a string")?;
        if quote != b'\'' && quote != b'"' {
            return Err("expected a quoted string".to_owned());
        }
        self.index += 1;
        let start = self.index;
        while self.text.get(self.index).is_some_and(|&c| c != quote) {
            self.index += 1;
        }
        let value = String::from_utf8_lossy(&self.text[start..self.index]).into_owned();
        self.index += 1;
        Ok(Expression::Compare { item, property, value, equal })
    }
}

fn parse_condition(text: &str) -> Result<Expression> {
    let mut parser = Parser { text: text.as_bytes(), index: 0 };
    let expression = parser.or()?;
    parser.skip();
    if parser.index != text.len() {
        return Err(format!("unexpected text in condition {text:?}"));
    }
    Ok(expression)
}

impl Rules {
    /// Rules from the project JSON text and an optional `.kicad_dru` text.
    pub fn load(project: &str, custom: Option<&str>) -> Result<Rules> {
        let document =
            copper_json::parse(project).map_err(|failure| format!("project: {}", failure.python_message(project)))?;
        let settings = document.get("board").and_then(|board| board.get("design_settings"));
        let rules = settings.and_then(|settings| settings.get("rules"));
        let rule = |name: &str| mm(rules.and_then(|rules| rules.get(name)), name);
        let required = |name: &str| -> Result<i64> {
            rule(name)?.ok_or_else(|| format!("project has no board.design_settings.rules.{name}"))
        };
        let mut class_clearance = IndexMap::new();
        let net_settings = document.get("net_settings");
        for class in net_settings.and_then(|n| n.get("classes")).and_then(Value::as_array).unwrap_or_default() {
            let name = class.get("name").and_then(Value::as_str).unwrap_or("Default").to_owned();
            if let Some(clearance) = mm(class.get("clearance"), "net class clearance")? {
                class_clearance.insert(name, clearance);
            }
        }
        let mut assignments = IndexMap::new();
        if let Some(Value::Object(map)) = net_settings.and_then(|n| n.get("netclass_assignments")) {
            for (net, class) in map {
                let class = match class {
                    Value::Str(class) => class.clone(),
                    Value::Array(classes) => classes.first().and_then(Value::as_str).unwrap_or("Default").to_owned(),
                    _ => continue,
                };
                assignments.insert(net.clone(), class);
            }
        }
        let mut patterns = Vec::new();
        for pattern in
            net_settings.and_then(|n| n.get("netclass_patterns")).and_then(Value::as_array).unwrap_or_default()
        {
            if let (Some(glob), Some(class)) =
                (pattern.get("pattern").and_then(Value::as_str), pattern.get("netclass").and_then(Value::as_str))
            {
                patterns.push((glob.to_owned(), class.to_owned()));
            }
        }
        let mut severities = IndexMap::new();
        if let Some(Value::Object(map)) = settings.and_then(|s| s.get("rule_severities")) {
            for (kind, severity) in map {
                if let Some(severity) = severity.as_str() {
                    severities.insert(kind.clone(), severity.to_owned());
                }
            }
        }
        let (custom, other_constraints) = match custom {
            Some(text) => parse_custom(text)?,
            None => (Vec::new(), Vec::new()),
        };
        Ok(Rules {
            min_clearance: rule("min_clearance")?.unwrap_or(0),
            edge_clearance: required("min_copper_edge_clearance")?,
            hole_clearance: required("min_hole_clearance")?,
            max_error: mm(settings.and_then(|s| s.get("max_error")), "max_error")?.unwrap_or(5_000),
            min_track_width: rule("min_track_width")?.unwrap_or(0),
            min_via_diameter: rule("min_via_diameter")?.unwrap_or(0),
            min_via_annular_width: rule("min_via_annular_width")?.unwrap_or(0),
            min_through_hole_diameter: rule("min_through_hole_diameter")?.unwrap_or(0),
            min_hole_to_hole: rule("min_hole_to_hole")?.unwrap_or(0),
            severities,
            other_constraints,
            class_clearance,
            assignments,
            patterns,
            custom,
        })
    }

    /// Whether a violation type is not reported: set to `ignore` in the
    /// project, or ignored by KiCad 10's defaults when the project is silent.
    pub fn ignores(&self, kind: &str) -> bool {
        /// KiCad 10 types whose default severity is `ignore`.
        const DEFAULT_IGNORED: &[&str] = &["track_not_centered_on_via"];
        match self.severities.get(kind) {
            Some(severity) => severity == "ignore",
            None => DEFAULT_IGNORED.contains(&kind),
        }
    }

    pub fn net_class<'a>(&'a self, net: Option<&'a str>) -> &'a str {
        let Some(net) = net else { return "Default" };
        if let Some(class) = self.assignments.get(net) {
            return class;
        }
        self.patterns.iter().find(|(pattern, _)| wildcard(pattern, net)).map_or("Default", |(_, class)| class)
    }

    fn class_clearance(&self, net: Option<&str>) -> i64 {
        let class = self.net_class(net);
        self.class_clearance.get(class).or_else(|| self.class_clearance.get("Default")).copied().unwrap_or(0)
    }

    fn evaluate(&self, expression: &Expression, a: &Item, b: &Item) -> bool {
        match expression {
            Expression::Or(left, right) => self.evaluate(left, a, b) || self.evaluate(right, a, b),
            Expression::And(left, right) => self.evaluate(left, a, b) && self.evaluate(right, a, b),
            Expression::Compare { item, property, value, equal } => {
                let subject = if *item == 'A' { a } else { b };
                let text = match property.as_str() {
                    "NetName" => subject.net.unwrap_or(""),
                    "NetClass" => self.net_class(subject.net),
                    _ => subject.reference.unwrap_or(""),
                };
                wildcard(value, text) == *equal
            }
        }
    }

    /// The last matching custom rule of `kind`, either order of A and B.
    fn custom(&self, kind: Kind, a: &Item, b: &Item) -> Option<i64> {
        self.custom
            .iter()
            .rev()
            .filter(|rule| rule.kind == kind)
            .find(|rule| {
                rule.condition
                    .as_ref()
                    .is_none_or(|condition| self.evaluate(condition, a, b) || self.evaluate(condition, b, a))
            })
            .map(|rule| rule.min)
    }

    /// Electrical clearance between a zone and an item.
    pub fn clearance(&self, zone: &Item, item: &Item) -> i64 {
        let side = |item: &Item| item.local_clearance.unwrap_or_else(|| self.class_clearance(item.net));
        let implicit = side(zone).max(side(item));
        self.min_clearance.max(implicit).max(self.custom(Kind::Clearance, zone, item).unwrap_or(0))
    }

    pub fn hole_clearance(&self, zone: &Item, item: &Item) -> i64 {
        self.hole_clearance.max(self.custom(Kind::HoleClearance, zone, item).unwrap_or(0))
    }

    pub fn edge_clearance(&self, zone: &Item) -> i64 {
        self.edge_clearance.max(self.custom(Kind::EdgeClearance, zone, &Item::default()).unwrap_or(0))
    }

    /// The largest clearance any rule can produce, for search margins.
    pub fn worst_clearance(&self) -> i64 {
        let classes = self.class_clearance.values().copied().max().unwrap_or(0);
        let custom = self.custom.iter().map(|rule| rule.min).max().unwrap_or(0);
        [self.min_clearance, self.edge_clearance, self.hole_clearance, classes, custom].into_iter().max().unwrap_or(0)
    }
}

fn strip_comments(text: &str) -> String {
    text.lines().map(|line| if line.trim_start().starts_with('#') { "" } else { line }).collect::<Vec<_>>().join("\n")
}

/// Constraints that change zone fills beyond the modelled clearances.
const REFUSED: &[&str] = &[
    "physical_clearance",
    "physical_hole_clearance",
    "thermal_relief_gap",
    "thermal_spoke_width",
    "zone_connection",
    "disallow",
];

/// Rule name and constraint type of a constraint that is not modelled.
type Other = (String, String);

/// The modelled clearance rules, and every other constraint by rule name.
fn parse_custom(text: &str) -> Result<(Vec<Rule>, Vec<Other>)> {
    let wrapped = format!("(rules\n{}\n)", strip_comments(text));
    let root = kicad_mod::parse_sexpr(&wrapped, ".kicad_dru").map_err(|failure| failure.to_string())?;
    let mut rules = Vec::new();
    let mut others = Vec::new();
    for item in root.iter().skip(1) {
        let Node::List(rule) = item else { continue };
        if kicad_mod::atom(rule, 0) != Some("rule") {
            continue;
        }
        let name = kicad_mod::atom(rule, 1).unwrap_or_default().to_owned();
        let constraints: Vec<&List> = kicad_mod::children(rule, "constraint");
        let relevant: Vec<(&List, Kind)> = constraints
            .iter()
            .filter_map(|constraint| {
                let kind = match kicad_mod::atom(constraint, 1)? {
                    "clearance" => Kind::Clearance,
                    "hole_clearance" => Kind::HoleClearance,
                    "edge_clearance" => Kind::EdgeClearance,
                    _ => return None,
                };
                Some((*constraint, kind))
            })
            .collect();
        if let Some(refused) =
            constraints.iter().filter_map(|c| kicad_mod::atom(c, 1)).find(|kind| REFUSED.contains(kind))
        {
            return Err(format!("custom rule {name:?}: {refused} constraints are not modelled by the native filler"));
        }
        for kind in constraints.iter().filter_map(|c| kicad_mod::atom(c, 1)) {
            if !matches!(kind, "clearance" | "hole_clearance" | "edge_clearance") {
                others.push((name.clone(), kind.to_owned()));
            }
        }
        if relevant.is_empty() {
            continue;
        }
        if kicad_mod::first(rule, "layer").is_some() {
            return Err(format!(
                "custom rule {name:?}: layer-qualified clearance rules are not modelled by the native filler"
            ));
        }
        let condition = match kicad_mod::first(rule, "condition").and_then(|condition| kicad_mod::atom(condition, 1)) {
            None => None,
            Some(text) => Some(parse_condition(text).map_err(|failure| format!("custom rule {name:?}: {failure}"))?),
        };
        for (constraint, kind) in relevant {
            let min = kicad_mod::first(constraint, "min")
                .and_then(|min| kicad_mod::atom(min, 1))
                .ok_or_else(|| format!("custom rule {name:?}: clearance without (min)"))?;
            let value = min.strip_suffix("mm").unwrap_or(min);
            let min = parse_nm(value).ok_or_else(|| format!("custom rule {name:?}: bad value {min:?}"))?;
            rules.push(Rule { kind, min, condition: condition.clone() });
        }
    }
    Ok((rules, others))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.15,
      "min_copper_edge_clearance": 0.3, "min_hole_clearance": 0.25}}},
      "net_settings": {"classes": [{"name": "Default", "clearance": 0.15}, {"name": "HV", "clearance": 0.5}],
      "netclass_assignments": {"MAINS": "HV"}, "netclass_patterns": [{"netclass": "HV", "pattern": "PWR_*"}]}}"#;

    const DRU: &str = "(version 1)\n# comment\n(rule \"Courtyard\" (constraint courtyard_clearance (min 0.5mm)))\n\
(rule \"Analog\" (constraint clearance (min 0.2mm)) (condition \"A.NetName == 'ADC' || B.NetName == 'ADC'\"))";

    fn item(net: &'static str) -> Item<'static> {
        Item { net: Some(net), ..Item::default() }
    }

    #[test]
    fn clearances_take_classes_rules_and_local_overrides() {
        let rules = Rules::load(PROJECT, Some(DRU)).unwrap();
        assert_eq!((rules.edge_clearance, rules.hole_clearance, rules.max_error), (300_000, 250_000, 5_000));
        assert_eq!(rules.other_constraints, vec![("Courtyard".to_owned(), "courtyard_clearance".to_owned())]);
        assert_eq!(rules.clearance(&item("GND"), &item("SIG")), 150_000);
        assert_eq!(rules.clearance(&item("GND"), &item("MAINS")), 500_000);
        assert_eq!(rules.clearance(&item("GND"), &item("PWR_24V")), 500_000);
        assert_eq!(rules.clearance(&item("GND"), &item("ADC")), 200_000);
        assert_eq!(rules.clearance(&item("ADC"), &item("GND")), 200_000);
        let zone = Item { net: Some("GND"), reference: None, local_clearance: Some(90_000) };
        assert_eq!(rules.clearance(&zone, &item("SIG")), 150_000);
        let wide = Item { local_clearance: Some(300_000), ..zone };
        assert_eq!(rules.clearance(&wide, &item("SIG")), 300_000);
    }

    #[test]
    fn unmodelled_constraints_are_refused() {
        let thermal = "(rule \"T\" (constraint thermal_relief_gap (min 0.3mm)))";
        assert!(Rules::load(PROJECT, Some(thermal)).unwrap_err().contains("thermal_relief_gap"));
        let layered = "(rule \"L\" (layer outer) (constraint clearance (min 0.3mm)))";
        assert!(Rules::load(PROJECT, Some(layered)).unwrap_err().contains("layer-qualified"));
        let unknown = "(rule \"U\" (constraint clearance (min 0.3mm)) (condition \"A.Type == 'Pad'\"))";
        assert!(Rules::load(PROJECT, Some(unknown)).unwrap_err().contains("not supported"));
    }

    #[test]
    fn wildcards_match_case_insensitively() {
        assert!(wildcard("PWR_*", "pwr_24v"));
        assert!(wildcard("A?C", "ABC"));
        assert!(!wildcard("A?C", "ABBC"));
        assert!(wildcard("*", ""));
    }
}

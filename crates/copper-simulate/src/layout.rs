//! `use layout;`: a bench with the parasitics of the routed board ([`LayoutParasitics`], extracted by
//! `copper_extract::parasitics` and handed in by `copper sim --layout RUN`).
//!
//! What goes into the circuit: each networked net (a net that is not a supply's) becomes the nodes of its routed
//! copper, every component pin on the node of its pad, and between them the tracks' resistance and inductance in
//! series; the net's name stays the node of one pad ([`LayoutNet::named`]), so `V(NET)` reads that pad and
//! `V(REF.PIN)` reads the pin's own pad. The extracted capacitances join those nodes to each other and to the
//! nodes of every other net (supply and ground nets are one node each). An element whose nets the bench does not
//! simulate (every pin on them ignored) is left out, so that no node floats.
//!
//! A bench that uses layout runs twice: as the schematic alone and with the layout. Its measures, plots and limits
//! are the run with the layout; [`LayoutRun`] keeps the schematic's measures beside them, and the largest extracted
//! contributors.

use std::sync::Arc;

use copper_lang::expression::Units;
use copper_units::Dimension;

use crate::format::quantity_text;
use crate::run::{BenchRun, MeasureValue};

/// A series branch of a net's network.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutBranch {
    pub a: usize,
    pub b: usize,
    /// Ohm.
    pub resistance: f64,
    /// Henry.
    pub inductance: f64,
    /// Metres of track.
    pub length_m: f64,
}

/// A net of the routed board, in the design's net names.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutNet {
    pub net: String,
    /// What each node is (`pad R1.2`, `track end F.Cu 12.00,4.50 mm`); one node for a net that is not networked.
    pub nodes: Vec<String>,
    /// The node that keeps the net's name.
    pub named: usize,
    /// `(component, pin, node)`: the design's component pins on the net.
    pub pins: Vec<(String, String, usize)>,
    pub branches: Vec<LayoutBranch>,
    pub track_length_m: f64,
    /// Capacitance to nothing within reach (not inserted), F.
    pub unattributed: f64,
}

/// A capacitance between a node of one net and a node of another (`(net index, node)`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutCapacitor {
    pub a: (usize, usize),
    pub b: (usize, usize),
    pub farad: f64,
}

/// The parasitics of a routed board.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutParasitics {
    /// The routed board file it was extracted from.
    pub source: String,
    pub nets: Vec<LayoutNet>,
    pub capacitors: Vec<LayoutCapacitor>,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
}

/// What `copper sim` hands the benches that say `use layout;`.
#[derive(Clone, Debug, Default)]
pub enum LayoutInput {
    /// No `--layout`: such a bench is refused (`SIM118`).
    #[default]
    Absent,
    Ready(Arc<LayoutParasitics>),
    /// The routed board cannot be used: the message (`SIM118` or `SIM119`) is each such bench's error.
    Refused(String),
}

/// The message of a bench that uses layout with no routed board given.
pub fn missing(bench: &str) -> String {
    format!(
        "SIM118: bench {bench} says use layout, but no routed board was given: route the board (copper route-board BOARD --checkpoint-dir RUN) and run copper sim BOARD --layout RUN; a bench that uses layout never runs on the schematic alone"
    )
}

impl LayoutParasitics {
    /// The circuit node of `node` of net `net` (index).
    pub fn node_name(&self, net: usize, node: usize) -> String {
        let n = &self.nets[net];
        if node == n.named || n.nodes.len() <= 1 { n.net.clone() } else { format!("{}#layout{node}", n.net) }
    }

    /// The resistors, inductors and capacitors the layout holds.
    pub fn element_counts(&self) -> (usize, usize, usize) {
        let branches = self.nets.iter().flat_map(|n| &n.branches);
        let resistors = branches.clone().filter(|b| b.resistance > 0.0).count();
        let inductors = branches.filter(|b| b.inductance > 0.0).count();
        (resistors, inductors, self.capacitors.len())
    }

    /// The largest contributors, largest first: net-to-net capacitances, then the series copper of each net with
    /// tracks.
    pub fn contributors(&self, limit: usize) -> Vec<Contributor> {
        let mut sums: std::collections::BTreeMap<(usize, usize), f64> = std::collections::BTreeMap::new();
        for c in &self.capacitors {
            *sums.entry((c.a.0.min(c.b.0), c.a.0.max(c.b.0))).or_default() += c.farad;
        }
        let mut capacitances: Vec<Contributor> = sums
            .into_iter()
            .map(|((i, j), farad)| Contributor {
                kind: "capacitance",
                nets: vec![self.nets[i].net.clone(), self.nets[j].net.clone()],
                value: farad,
                text: format!(
                    "{} to {}: {}",
                    self.nets[i].net,
                    self.nets[j].net,
                    quantity_text(farad, Units::of(Dimension::Capacitance), 3, false)
                ),
            })
            .collect();
        capacitances.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.nets.cmp(&b.nets)));
        capacitances.truncate(limit);
        let mut series: Vec<Contributor> = self
            .nets
            .iter()
            .filter(|n| !n.branches.is_empty())
            .map(|n| {
                let r: f64 = n.branches.iter().map(|b| b.resistance).sum();
                let l: f64 = n.branches.iter().map(|b| b.inductance).sum();
                Contributor {
                    kind: "series",
                    nets: vec![n.net.clone()],
                    value: l,
                    text: format!(
                        "{}: {} of track in {} branches, {} and {} in all",
                        n.net,
                        quantity_text(n.track_length_m, Units::of(Dimension::Length), 3, false),
                        n.branches.len(),
                        quantity_text(r, Units::of(Dimension::Resistance), 3, false),
                        quantity_text(l, Units::of(Dimension::Inductance), 3, false)
                    ),
                }
            })
            .collect();
        series.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.nets.cmp(&b.nets)));
        series.truncate(limit);
        capacitances.extend(series);
        capacitances
    }
}

/// One of the largest extracted values.
#[derive(Clone, Debug, PartialEq)]
pub struct Contributor {
    /// `capacitance` (between two nets) or `series` (a net's tracks, its total inductance as the value).
    pub kind: &'static str,
    pub nets: Vec<String>,
    /// F, or H for `series`.
    pub value: f64,
    pub text: String,
}

/// The layout side of a bench run with `use layout;`.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutRun {
    pub source: String,
    /// `(resistors, inductors, capacitors)` the extraction holds.
    pub elements: (usize, usize, usize),
    pub contributors: Vec<Contributor>,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    /// The schematic-only run's error, if it could not be solved.
    pub schematic_error: Option<String>,
    /// The schematic-only run's measure values, `[step][measure]` (empty with an error).
    pub schematic: Vec<Vec<MeasureValue>>,
}

impl LayoutRun {
    pub fn new(layout: &LayoutParasitics, schematic: &BenchRun) -> LayoutRun {
        LayoutRun {
            source: layout.source.clone(),
            elements: layout.element_counts(),
            contributors: layout.contributors(5),
            flags: layout.flags.clone(),
            notes: layout.notes.clone(),
            schematic_error: schematic.error.clone(),
            schematic: if schematic.error.is_some() {
                Vec::new()
            } else {
                schematic.steps.iter().map(|step| step.measures.clone()).collect()
            },
        }
    }
}

/// An element the layout adds to a circuit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Element {
    Resistor,
    Inductor,
    Capacitor,
}

/// Every element of `layout` in circuit terms: `(name, kind, node a, node b, value, the net or nets it belongs to)`.
pub(crate) fn elements(layout: &LayoutParasitics) -> Vec<(String, Element, String, String, f64, Vec<usize>)> {
    let mut out = Vec::new();
    for (i, net) in layout.nets.iter().enumerate() {
        for (k, branch) in net.branches.iter().enumerate() {
            let (a, b) = (layout.node_name(i, branch.a), layout.node_name(i, branch.b));
            let name = format!("layout.{}.{k}", net.net);
            if branch.inductance > 0.0 && branch.resistance > 0.0 {
                let mid = format!("{}#layout{}-{}", net.net, branch.a, branch.b);
                let mid = format!("{mid}.{k}");
                out.push((format!("{name}.r"), Element::Resistor, a, mid.clone(), branch.resistance, vec![i]));
                out.push((format!("{name}.l"), Element::Inductor, mid, b, branch.inductance, vec![i]));
            } else if branch.resistance > 0.0 {
                out.push((format!("{name}.r"), Element::Resistor, a, b, branch.resistance, vec![i]));
            } else if branch.inductance > 0.0 {
                out.push((format!("{name}.l"), Element::Inductor, a, b, branch.inductance, vec![i]));
            }
        }
    }
    for (k, c) in layout.capacitors.iter().enumerate() {
        out.push((
            format!("layout.c{k}"),
            Element::Capacitor,
            layout.node_name(c.a.0, c.a.1),
            layout.node_name(c.b.0, c.b.1),
            c.farad,
            vec![c.a.0, c.b.0],
        ));
    }
    out
}

/// The layout lines of the text table.
pub fn text_lines(run: &BenchRun) -> Vec<String> {
    let Some(layout) = &run.layout else { return Vec::new() };
    let mut lines = Vec::new();
    let (r, l, c) = layout.elements;
    lines.push(format!("  layout {}: {r} resistors, {l} inductors, {c} capacitors", layout.source));
    let largest: Vec<&str> =
        layout.contributors.iter().filter(|c| c.kind == "capacitance").take(3).map(|c| c.text.as_str()).collect();
    if !largest.is_empty() {
        lines.push(format!("  layout largest: {}", largest.join("; ")));
    }
    if let Some(error) = &layout.schematic_error {
        lines.push(format!("  schematic alone: ERROR: {error}"));
        return lines;
    }
    for (k, measure) in run.measures.iter().enumerate() {
        let step = measure.worst.unwrap_or(0);
        let (Some(with), Some(without)) =
            (run.steps.get(step).and_then(|s| s.measures.get(k)), layout.schematic.get(step).and_then(|s| s.get(k)))
        else {
            continue;
        };
        let at = if run.steps.len() > 1 { format!(" at {}", run.steps[step].label) } else { String::new() };
        lines.push(format!(
            "  layout {}: {} schematic, {} with layout{}{at}",
            measure.name,
            crate::results::measure_text(without.value, measure.units),
            crate::results::measure_text(with.value, measure.units),
            change(without.value, with.value)
        ));
    }
    lines
}

/// ` (+3.2 %)`, or nothing where a relative change says nothing.
fn change(without: f64, with: f64) -> String {
    if !(without.is_finite() && with.is_finite()) || without == 0.0 {
        return String::new();
    }
    let relative = (with - without) / without.abs();
    format!(" ({}{} %)", if relative >= 0.0 { "+" } else { "" }, crate::format::significant(100.0 * relative, 3))
}

/// The `layout` object of a bench in the results JSON.
pub fn value(run: &BenchRun, layout: &LayoutRun) -> copper_json::Value {
    use copper_json::{Object, Value};
    let number = |x: f64| if x.is_finite() { Value::from(x) } else { Value::Null };
    let object = |entries: Vec<(&str, Value)>| -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Object>())
    };
    let contributors = layout
        .contributors
        .iter()
        .map(|c| {
            object(vec![
                ("kind", Value::from(c.kind)),
                ("nets", Value::Array(c.nets.iter().map(|n| Value::from(n.as_str())).collect())),
                ("value", number(c.value)),
                ("unit", Value::from(if c.kind == "capacitance" { "F" } else { "H" })),
                ("text", Value::from(c.text.as_str())),
            ])
        })
        .collect();
    let measures = run
        .measures
        .iter()
        .enumerate()
        .map(|(k, measure)| {
            let steps = layout
                .schematic
                .iter()
                .map(|step| {
                    let m = &step[k];
                    object(vec![
                        ("value", if m.never { Value::Null } else { number(m.value) }),
                        ("pass", m.pass.map(Value::Bool).unwrap_or(Value::Null)),
                    ])
                })
                .collect();
            object(vec![("name", Value::from(measure.name.as_str())), ("steps", Value::Array(steps))])
        })
        .collect();
    let (r, l, c) = layout.elements;
    object(vec![
        ("source", Value::from(layout.source.as_str())),
        (
            "elements",
            object(vec![
                ("resistors", Value::from(r as i64)),
                ("inductors", Value::from(l as i64)),
                ("capacitors", Value::from(c as i64)),
            ]),
        ),
        ("contributors", Value::Array(contributors)),
        ("flags", Value::Array(layout.flags.iter().map(|f| Value::from(f.as_str())).collect())),
        ("notes", Value::Array(layout.notes.iter().map(|n| Value::from(n.as_str())).collect())),
        (
            "schematic",
            object(vec![
                ("error", layout.schematic_error.as_deref().map(Value::from).unwrap_or(Value::Null)),
                ("measures", Value::Array(measures)),
            ]),
        ),
    ])
}

/// The bench page's layout section: schematic against layout per measure, and the contributors.
pub fn html_section(run: &BenchRun) -> String {
    use crate::report::escape;
    let Some(layout) = &run.layout else { return String::new() };
    let mut page = vec![format!(
        "<h2>Layout</h2>\n<p>Parasitics of the routed board {}: {} resistors, {} inductors, {} capacitors. The plots and limits above are the run with them.</p>",
        escape(&layout.source),
        layout.elements.0,
        layout.elements.1,
        layout.elements.2
    )];
    match &layout.schematic_error {
        Some(error) => page.push(format!("<p>The schematic alone could not be solved: {}</p>", escape(error))),
        None => {
            page.push(
                "<div class=\"scroll\"><table>\n<tr><th>measure</th><th>step</th><th>schematic</th><th>with layout</th><th>change</th></tr>"
                    .to_owned(),
            );
            for (k, measure) in run.measures.iter().enumerate() {
                for (s, step) in run.steps.iter().enumerate() {
                    let (Some(with), Some(without)) =
                        (step.measures.get(k), layout.schematic.get(s).and_then(|x| x.get(k)))
                    else {
                        continue;
                    };
                    page.push(format!(
                        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                        escape(&measure.name),
                        escape(&step.label),
                        escape(&crate::results::measure_text(without.value, measure.units)),
                        escape(&crate::results::measure_text(with.value, measure.units)),
                        escape(change(without.value, with.value).trim().trim_start_matches('(').trim_end_matches(')'))
                    ));
                }
            }
            page.push("</table></div>".to_owned());
        }
    }
    page.push("<h3>Largest extracted values</h3>\n<ul>".to_owned());
    for c in &layout.contributors {
        page.push(format!("<li>{}</li>", escape(&c.text)));
    }
    page.push("</ul>".to_owned());
    for note in &layout.notes {
        page.push(format!("<p class=\"note\">{}</p>", escape(note)));
    }
    page.join("\n")
}

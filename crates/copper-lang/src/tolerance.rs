//! Tolerances, matched groups, Monte Carlo and worst-case runs of benches
//! (`docs/language-reference.md`, "Tolerances, Monte Carlo and worst case").
//!
//! A component declares its value tolerance and, for a resistor network or
//! another ratio-matched set, the group it is matched in:
//!
//! ```copper
//! component R1: RESISTOR { value = 10kohm; tolerance = 0.1%; match = "HOWLAND"; match_tolerance = 0.01%; }
//! ```
//!
//! A bench adds tolerances of params, of model parameters and of
//! components that declare none (`tolerance R_SET = 0.5%;`,
//! `tolerance U1.input_offset_voltage = 25uV;`, `tolerance C1 = 10%;`),
//! matched groups of passives and model parameters (`tolerance
//! RN.resistance = 7.5% match 0.0125%;`: every unit of `RN` one member), and
//! one method that uses them: `montecarlo { runs = 200; seed = 1;
//! distribution = gaussian; }` or `worstcase;`. Without a method a bench runs
//! at the nominal values and the declared tolerances change nothing.
//!
//! Every toleranced quantity becomes one or more variables `x` in `[-1, 1]`
//! ([`Variation::variables`]): `x = +1` is the quantity at its upper limit.
//! A matched group's members share one common variable and have one own
//! variable each: member `i` is `nominal (1 + c x_c + m x_i)` with
//! `m = match_tolerance / 2` and `c = tolerance - m`, so every member stays
//! within its tolerance and every ratio of two members within
//! `match_tolerance`. Nothing here samples or simulates; `copper-simulate` does.

use copper_json::Value;
use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;

use crate::bench::{BenchModel, BenchParam, Replacement, SimComponent, Treatment};
use crate::error::CopperScriptError;
use crate::expression::sim_quantity;
use crate::model::{ComponentInstance, PartDefinition};
use crate::serializer::quantity_value;
use crate::simulation::{SimKind, SimSpec, SourceOutput};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

/// The component attribute of a value tolerance (`tolerance = 1%;`).
pub const TOLERANCE: &str = "tolerance";
/// The component attribute naming a matched group (`match = "NETWORK";`).
pub const MATCH: &str = "match";
/// The component attribute of a matched group's ratio tolerance (`match_tolerance = 0.01%;`).
pub const MATCH_TOLERANCE: &str = "match_tolerance";

/// The largest number of Monte Carlo runs.
pub const MAX_RUNS: u32 = 100_000;

/// Model parameters whose nominal value is zero when a part does not
/// declare them, so that an absolute tolerance may vary them anyway.
const ZERO_DEFAULT: [&str; 3] = ["input_offset_voltage", "input_bias_current", "input_offset_current"];

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

fn suggesting(mut failure: CopperScriptError, word: &str, candidates: &[&str]) -> CopperScriptError {
    if let Some(suggestion) = close_match(word, candidates) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    failure
}

/// A percentage strictly between 0 % and 100 %, for `label`.
fn percentage(value: &Scalar, label: &str, location: &Location) -> Result<Quantity> {
    let quantity = match value {
        Scalar::Quantity { value, unit } if unit == "%" => sim_quantity(value, unit),
        _ => None,
    };
    // 100 % is the base value 1.
    let whole = Decimal::from_i64(1);
    match quantity {
        Some(q) if !q.base_value().is_zero() && !q.base_value().is_negative() && *q.base_value() < whole => Ok(q),
        _ => Err(error(
            "SIM089",
            format!("{label} must be a percentage above 0 % and below 100 %, such as 1%"),
            location,
        )),
    }
}

/// Check a component's `tolerance`, `match` and `match_tolerance`; the
/// properties to store on the compiled component (empty when none is declared).
pub(crate) fn component_properties(attributes: &Attributes, location: &Location) -> Result<Vec<(String, String)>> {
    let mut properties = Vec::new();
    let tolerance = attributes.get(TOLERANCE).map(|value| percentage(value, "component tolerance", location));
    let tolerance = tolerance.transpose()?;
    if let Some(tolerance) = &tolerance {
        properties.push((TOLERANCE.to_owned(), tolerance.to_string()));
    }
    let group = match attributes.get(MATCH) {
        None => None,
        Some(Scalar::Str(name)) if !name.trim().is_empty() => Some(name.clone()),
        Some(_) => {
            return Err(error("SIM089", "component match must name a matched group: match = \"NETWORK\";", location));
        }
    };
    let match_tolerance =
        attributes.get(MATCH_TOLERANCE).map(|value| percentage(value, "component match_tolerance", location));
    let match_tolerance = match_tolerance.transpose()?;
    match (&group, &match_tolerance, &tolerance) {
        (None, None, _) => {}
        (Some(_), Some(m), Some(t)) => {
            if m.base_value() > t.base_value() {
                return Err(error(
                    "SIM089",
                    format!(
                        "match_tolerance {m} is above the tolerance {t}; a ratio cannot spread more than its parts"
                    ),
                    location,
                ));
            }
        }
        (Some(_), Some(_), None) => {
            return Err(error(
                "SIM089",
                "a matched component needs its absolute tolerance too: tolerance = 0.1%;",
                location,
            ));
        }
        (Some(_), None, _) => {
            return Err(error("SIM089", "a matched component needs match_tolerance, its ratio tolerance", location));
        }
        (None, Some(_), _) => {
            return Err(error("SIM089", "match_tolerance needs match, the name of the matched group", location));
        }
    }
    if let (Some(group), Some(m)) = (group, match_tolerance) {
        properties.push((MATCH.to_owned(), group));
        properties.push((MATCH_TOLERANCE.to_owned(), m.to_string()));
    }
    Ok(properties)
}

/// A stored property's quantity (`"0.1 %"`).
fn stored(text: &str) -> Option<Quantity> {
    let (number, unit) = text.rsplit_once(' ')?;
    sim_quantity(number, unit)
}

/// How Monte Carlo runs draw a variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distribution {
    /// Uniform over the tolerance band.
    Uniform,
    /// Normal with the tolerance at three standard deviations, truncated at the tolerance.
    Gaussian,
}

impl Distribution {
    pub fn name(self) -> &'static str {
        match self {
            Distribution::Uniform => "uniform",
            Distribution::Gaussian => "gaussian",
        }
    }
}

/// What a bench does with its tolerances.
#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    /// `montecarlo { runs; seed; distribution; }`.
    MonteCarlo { runs: u32, seed: u64, distribution: Distribution },
    /// `worstcase;`: every variable at the end of its range that moves each measure up, then down.
    WorstCase,
}

impl Method {
    pub fn name(&self) -> &'static str {
        match self {
            Method::MonteCarlo { .. } => "montecarlo",
            Method::WorstCase => "worstcase",
        }
    }
}

/// What a tolerance varies.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A passive component's value.
    Component(String),
    /// A bench or board param (not a derived one).
    Param(String),
    /// A parameter of a component's `sim` blocks, of a replacement or of a
    /// bench model; on a multi-unit part every unit varies on its own.
    ModelParameter { instance: String, parameter: String },
}

impl Target {
    /// The target as written: `R1`, `R_SET`, `U1.gbw`.
    pub fn text(&self) -> String {
        match self {
            Target::Component(name) | Target::Param(name) => name.clone(),
            Target::ModelParameter { instance, parameter } => format!("{instance}.{parameter}"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Target::Component(_) => "component",
            Target::Param(_) => "param",
            Target::ModelParameter { .. } => "model_parameter",
        }
    }
}

/// The size of a tolerance.
#[derive(Clone, Debug, PartialEq)]
pub enum Spread {
    /// A fraction of the nominal value (`1%`).
    Relative(Quantity),
    /// An amount of the quantity's own dimension (`25uV`, `3dB`).
    Absolute(Quantity),
}

impl Spread {
    pub fn quantity(&self) -> &Quantity {
        match self {
            Spread::Relative(q) | Spread::Absolute(q) => q,
        }
    }
}

/// One toleranced quantity outside matched groups.
#[derive(Clone, Debug, PartialEq)]
pub struct Tolerance {
    pub target: Target,
    pub spread: Spread,
    /// `component` when the component declares it, `bench` for a `tolerance` statement.
    pub declared_by: &'static str,
    pub location: Location,
}

/// A ratio-matched group (`match = "NAME"` on components, or a bench
/// `tolerance A, B = t match m;`): passive values and model parameters (one
/// member per unit of a multi-unit part).
#[derive(Clone, Debug, PartialEq)]
pub struct MatchGroup {
    pub name: String,
    pub members: Vec<Target>,
    /// Every member's absolute tolerance.
    pub tolerance: Quantity,
    /// The tolerance of the ratio of any two members.
    pub match_tolerance: Quantity,
}

/// The tolerances of a bench with a method.
#[derive(Clone, Debug, PartialEq)]
pub struct Variation {
    pub method: Method,
    pub location: Location,
    pub tolerances: Vec<Tolerance>,
    pub groups: Vec<MatchGroup>,
}

/// How one variable moves its quantity: `nominal * (1 + scale x)` or `nominal + scale x`.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Relative: `nominal * (1 + scale x)` for each target (a group's common
    /// variable moves every member).
    Relative { targets: Vec<Target>, scale: f64 },
    /// Absolute, in the quantity's SI base unit: `nominal + scale x`.
    Absolute { target: Target, scale: f64 },
}

/// One independent variable of a varied bench.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    /// `R1`, `U1.input_offset_voltage`, `HOWLAND common`, `HOWLAND R3`.
    pub name: String,
    pub effect: Effect,
    /// The matched group the variable belongs to (its common or a member's own variable).
    pub group: Option<String>,
}

fn base(quantity: &Quantity) -> f64 {
    quantity.base_value().to_sci_string().parse().unwrap_or(f64::NAN)
}

impl Variation {
    /// The independent variables, in declaration order: matched groups
    /// first (common variable, then one per member), then the other tolerances.
    pub fn variables(&self) -> Vec<Variable> {
        let mut out = Vec::new();
        for group in &self.groups {
            let half = base(&group.match_tolerance) / 2.0;
            let common = base(&group.tolerance) - half;
            let members: Vec<Target> = group.members.clone();
            if common > 0.0 {
                out.push(Variable {
                    name: format!("{} common", group.name),
                    effect: Effect::Relative { targets: members.clone(), scale: common },
                    group: Some(group.name.clone()),
                });
            }
            for member in members {
                out.push(Variable {
                    name: format!("{} {}", group.name, member.text()),
                    effect: Effect::Relative { targets: vec![member], scale: half },
                    group: Some(group.name.clone()),
                });
            }
        }
        for tolerance in &self.tolerances {
            let effect = match &tolerance.spread {
                Spread::Relative(q) => Effect::Relative { targets: vec![tolerance.target.clone()], scale: base(q) },
                Spread::Absolute(q) => Effect::Absolute { target: tolerance.target.clone(), scale: base(q) },
            };
            out.push(Variable { name: tolerance.target.text(), effect, group: None });
        }
        out
    }
}

/// Where the bench's components and models are, for resolving targets.
pub(crate) struct Lookup<'a> {
    pub components: &'a [SimComponent],
    pub instances: Vec<&'a ComponentInstance>,
    pub params: &'a IndexMap<String, BenchParam>,
    pub replacements: &'a [Replacement],
    pub models: &'a [BenchModel],
    pub part: &'a dyn Fn(&str) -> Option<&'a PartDefinition>,
}

impl Lookup<'_> {
    fn instance(&self, reference: &str) -> Option<&ComponentInstance> {
        self.instances.iter().copied().find(|component| component.reference == reference)
    }

    /// The model specs a component or bench model simulates with.
    fn specs(&self, name: &str) -> Option<Vec<&SimSpec>> {
        if let Some(model) = self.models.iter().find(|model| model.name == name) {
            return Some(vec![&model.spec]);
        }
        let component = self.components.iter().find(|component| component.reference == name)?;
        match &component.treatment {
            Treatment::Replaced(index) => Some(vec![&self.replacements[*index].spec]),
            Treatment::PartModel => {
                let part = (self.part)(&component.part)?;
                Some(part.sim.iter().map(|model| &model.spec).collect())
            }
            _ => None,
        }
    }
}

/// The dimension of model parameter `key` of `spec`, if the kind has it.
fn parameter_dimension(spec: &SimSpec, key: &str) -> Option<Dimension> {
    if spec.kind == SimKind::Source && (key == "dc" || key == "ac") {
        return Some(spec.output.unwrap_or(SourceOutput::Voltage).dimension());
    }
    spec.kind.parameters().iter().find(|(name, _)| *name == key).map(|(_, dimension)| *dimension)
}

/// Whether model parameter `key` of `spec` has a value to vary: declared, or
/// one of the parameters whose undeclared value is zero.
pub fn has_parameter(spec: &SimSpec, key: &str) -> bool {
    parameter_dimension(spec, key).is_some() && (spec.parameters.contains_key(key) || ZERO_DEFAULT.contains(&key))
}

/// The zero of an undeclared zero-default model parameter (`input_offset_voltage`).
pub fn zero_parameter(spec: &SimSpec, key: &str) -> Option<Quantity> {
    if !ZERO_DEFAULT.contains(&key) {
        return None;
    }
    let dimension = parameter_dimension(spec, key)?;
    Quantity::from_base(dimension, Decimal::zero(), dimension.units().first()?.0).ok()
}

/// A tolerance value for a quantity of `dimension`: `%` is relative, anything else absolute.
fn spread(value: &Scalar, dimension: Dimension, label: &str, location: &Location) -> Result<Spread> {
    let Scalar::Quantity { value: number, unit } = value else {
        return Err(error(
            "SIM091",
            format!("{label} takes a percentage or a {} quantity", dimension.name()),
            location,
        ));
    };
    if unit == "%" {
        if matches!(dimension, Dimension::Gain | Dimension::Angle | Dimension::Ratio) {
            return Err(error(
                "SIM091",
                format!("{label} is a {}; give its tolerance in its own unit, not in percent", dimension.name()),
                location,
            ));
        }
        return percentage(value, label, location).map(Spread::Relative);
    }
    match sim_quantity(number, unit) {
        Some(q) if q.dimension() == dimension && !q.base_value().is_negative() && !q.base_value().is_zero() => {
            Ok(Spread::Absolute(q))
        }
        Some(q) if q.dimension() == dimension => {
            Err(error("SIM091", format!("{label} must be positive, found {q}"), location))
        }
        _ => Err(error(
            "SIM091",
            format!("{label} must be a percentage or a {}, found {number}{unit}", dimension.name()),
            location,
        )),
    }
}

/// The method and tolerances of a bench document; `None` when it declares no method.
pub(crate) fn variation(document: &Document, lookup: &Lookup) -> Result<Option<Variation>> {
    let statements: Vec<&VariationDecl> = document
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Bench(BenchStatement::Variation(item)) => Some(item),
            _ => None,
        })
        .collect();
    let mut method: Option<(Method, Location)> = None;
    for item in statements.iter().filter(|item| item.kind != "tolerance") {
        if let Some((_, _)) = &method {
            return Err(error("SIM090", "a bench runs one of montecarlo and worstcase, once", &item.location));
        }
        let parsed = if item.kind == "worstcase" {
            if let Some(entry) = item.entries.first() {
                return Err(error("SIM090", "worstcase takes no properties", &entry.location));
            }
            Method::WorstCase
        } else {
            monte_carlo(item)?
        };
        method = Some((parsed, item.location.clone()));
    }
    let tolerance_statements: Vec<&&VariationDecl> =
        statements.iter().filter(|item| item.kind == "tolerance").collect();
    let Some((method, location)) = method else {
        if let Some(item) = tolerance_statements.first() {
            return Err(error(
                "SIM090",
                "a tolerance statement needs montecarlo { ... } or worstcase; in the bench to have an effect",
                &item.location,
            ));
        }
        return Ok(None);
    };
    // Bench tolerance statements, and the matched groups they declare.
    let mut bench: Vec<Tolerance> = Vec::new();
    let mut bench_groups: Vec<MatchGroup> = Vec::new();
    for item in tolerance_statements {
        let (Some(target), Some(value)) = (&item.target, &item.value) else { continue };
        if let Some(matched) = &item.match_value {
            let group = bench_group(target, &item.others, value, matched, &item.location, lookup)?;
            let taken = |member: &Target| {
                bench_groups.iter().any(|other| other.members.contains(member))
                    || bench.iter().any(|other| &other.target == member)
            };
            if let Some(member) = group.members.iter().find(|member| taken(member)) {
                return Err(error(
                    "SIM091",
                    format!("{} has two tolerance statements", repr(&member.text())),
                    &item.location,
                ));
            }
            bench_groups.push(group);
            continue;
        }
        if let Some(other) = item.others.first() {
            return Err(error(
                "SIM091",
                format!(
                    "tolerance {target}, {other} = ...; lists several targets, which makes a matched group: add its ratio tolerance, tolerance {target}, {other} = t match m;"
                ),
                &item.location,
            ));
        }
        let tolerance = bench_tolerance(target, value, &item.location, lookup)?;
        if bench_groups.iter().any(|group| group.members.contains(&tolerance.target)) {
            return Err(error("SIM091", format!("{} has two tolerance statements", repr(target)), &item.location));
        }
        if bench.iter().any(|other| other.target == tolerance.target) {
            return Err(error("SIM091", format!("{} has two tolerance statements", repr(target)), &item.location));
        }
        bench.push(tolerance);
    }
    // Component tolerances and matched groups.
    let mut tolerances: Vec<Tolerance> = Vec::new();
    let mut groups: Vec<MatchGroup> = Vec::new();
    for component in lookup.components {
        let Some(instance) = lookup.instance(&component.reference) else { continue };
        let declared = instance.properties.get(TOLERANCE).and_then(|text| stored(text));
        let Some(declared) = declared else { continue };
        let group = instance.properties.get(MATCH);
        let own = Target::Component(component.reference.clone());
        let overridden = bench.iter().find(|item| item.target == own);
        // A part with `load` blocks (a resistor network) varies the resistance of each of them.
        let members: Vec<Target> = match &component.treatment {
            Treatment::Ignored | Treatment::Replaced(_) => continue,
            Treatment::Passive { .. } => vec![own.clone()],
            Treatment::PartModel => {
                let units = load_resistances(component, lookup);
                if units.is_empty() {
                    return Err(error(
                        "SIM091",
                        format!(
                            "{} declares a tolerance, but it simulates with a sim model that has no load resistance; a component tolerance applies to the value of a resistor, capacitor or inductor or to the resistances of a part made of load blocks (vary a model parameter with tolerance {}.PARAMETER = ...;)",
                            repr(&component.reference),
                            component.reference
                        ),
                        &location,
                    ));
                }
                units
            }
        };
        if let Some(group) = bench_groups.iter().find(|group| members.iter().any(|m| group.members.contains(m))) {
            if let Some(name) = group_name(instance) {
                return Err(error(
                    "SIM091",
                    format!(
                        "{} is in matched group {} on its component; it cannot also be in the bench's group {}",
                        repr(&component.reference),
                        repr(name),
                        repr(&group.name)
                    ),
                    &location,
                ));
            }
            // The bench's group replaces the component's own tolerance.
            continue;
        }
        if matches!(component.treatment, Treatment::PartModel) {
            match group {
                Some(name) => {
                    let match_tolerance = instance
                        .properties
                        .get(MATCH_TOLERANCE)
                        .and_then(|text| stored(text))
                        .unwrap_or(declared.clone());
                    add_members(
                        &mut groups,
                        name,
                        members,
                        &declared,
                        &match_tolerance,
                        &component.reference,
                        &location,
                    )?;
                }
                None => tolerances.push(Tolerance {
                    target: Target::ModelParameter {
                        instance: component.reference.clone(),
                        parameter: "resistance".to_owned(),
                    },
                    spread: Spread::Relative(declared),
                    declared_by: "component",
                    location: location.clone(),
                }),
            }
            continue;
        }
        match (group, overridden) {
            (Some(name), Some(item)) => {
                return Err(error(
                    "SIM091",
                    format!(
                        "{} is in matched group {}; change the tolerances of the group on its components",
                        repr(&component.reference),
                        repr(name)
                    ),
                    &item.location,
                ));
            }
            (Some(name), None) => {
                let match_tolerance =
                    instance.properties.get(MATCH_TOLERANCE).and_then(|text| stored(text)).unwrap_or(declared.clone());
                add_members(&mut groups, name, members, &declared, &match_tolerance, &component.reference, &location)?;
            }
            (None, Some(_)) => {}
            (None, None) => tolerances.push(Tolerance {
                target: Target::Component(component.reference.clone()),
                spread: Spread::Relative(declared),
                declared_by: "component",
                location: location.clone(),
            }),
        }
    }
    if let Some(group) = groups.iter().find(|group| group.members.len() < 2) {
        return Err(error(
            "SIM092",
            format!(
                "matched group {} has one simulated member ({}); a match needs two or more components",
                repr(&group.name),
                group.members[0].text()
            ),
            &location,
        ));
    }
    tolerances.extend(bench);
    groups.extend(bench_groups);
    if tolerances.is_empty() && groups.is_empty() {
        return Err(error(
            "SIM093",
            format!(
                "{} has nothing to vary: no simulated component declares a tolerance and the bench has no tolerance statement",
                method.name()
            ),
            &location,
        ));
    }
    Ok(Some(Variation { method, location, tolerances, groups }))
}

/// The matched group a component's `match` names, if any.
fn group_name(instance: &ComponentInstance) -> Option<&String> {
    instance.properties.get(MATCH)
}

/// Add `members` of `component` to the component group `name` (`SIM092` on unequal tolerances).
fn add_members(
    groups: &mut Vec<MatchGroup>,
    name: &str,
    members: Vec<Target>,
    tolerance: &Quantity,
    match_tolerance: &Quantity,
    component: &str,
    location: &Location,
) -> Result<()> {
    match groups.iter_mut().find(|group| group.name == name) {
        Some(group) => {
            if &group.tolerance != tolerance || &group.match_tolerance != match_tolerance {
                return Err(error(
                    "SIM092",
                    format!(
                        "matched group {} has members with different tolerances: {component} has {tolerance} and match {match_tolerance}, {} has {} and match {}",
                        repr(name),
                        group.members[0].text(),
                        group.tolerance,
                        group.match_tolerance
                    ),
                    location,
                ));
            }
            group.members.extend(members);
        }
        None => groups.push(MatchGroup {
            name: name.to_owned(),
            members,
            tolerance: tolerance.clone(),
            match_tolerance: match_tolerance.clone(),
        }),
    }
    Ok(())
}

/// The simulator instance of each `sim` block of a component's part (`U1`,
/// or `U1.A` on a multi-unit part) with its spec.
fn unit_specs<'a>(component: &SimComponent, lookup: &Lookup<'a>) -> Vec<(String, &'a SimSpec)> {
    let Some(part) = (lookup.part)(&component.part) else { return Vec::new() };
    // The blocks one run uses: a unit's macro and vendor models are one instance.
    let blocks = crate::simulation::select_models(&part.sim, crate::simulation::ModelChoice::Default);
    let count = blocks.len();
    blocks
        .into_iter()
        .map(|block| {
            let instance = match (&block.unit, count) {
                (Some(unit), n) if n > 1 => format!("{}.{unit}", component.reference),
                _ => component.reference.clone(),
            };
            (instance, &block.spec)
        })
        .collect()
}

/// The `resistance` of every `load` block of a part-model component that declares one.
fn load_resistances(component: &SimComponent, lookup: &Lookup) -> Vec<Target> {
    unit_specs(component, lookup)
        .into_iter()
        .filter(|(_, spec)| spec.kind == SimKind::Load && spec.parameters.contains_key("resistance"))
        .map(|(instance, _)| Target::ModelParameter { instance, parameter: "resistance".to_owned() })
        .collect()
}

/// A bench `tolerance A, B = t match m;`: a matched group of passives and
/// model parameters (`COMPONENT.PARAMETER` is one member per unit that
/// declares it; `COMPONENT.UNIT.PARAMETER` one unit).
fn bench_group(
    first: &str,
    others: &[String],
    value: &Scalar,
    matched: &Scalar,
    location: &Location,
    lookup: &Lookup,
) -> Result<MatchGroup> {
    let tolerance = percentage(value, "a matched tolerance", location).map_err(|_| {
        error("SIM091", "a matched group's tolerance is a percentage: tolerance A, B = 1% match 0.1%;", location)
    })?;
    let match_tolerance = percentage(matched, "a match tolerance", location)
        .map_err(|_| error("SIM092", "a matched group's ratio tolerance is a percentage: ... match 0.1%;", location))?;
    if match_tolerance.base_value() > tolerance.base_value() {
        return Err(error(
            "SIM092",
            format!(
                "match {match_tolerance} is above the tolerance {tolerance}; a ratio cannot spread more than its parts"
            ),
            location,
        ));
    }
    let mut members: Vec<Target> = Vec::new();
    for target in std::iter::once(first).chain(others.iter().map(String::as_str)) {
        let found = group_members(target, location, lookup)?;
        for member in found {
            if members.contains(&member) {
                return Err(error(
                    "SIM092",
                    format!("{} is in the matched group twice", repr(&member.text())),
                    location,
                ));
            }
            members.push(member);
        }
    }
    if members.len() < 2 {
        return Err(error(
            "SIM092",
            format!("tolerance {first} ... match: the group has {} member; a match needs two or more", members.len()),
            location,
        ));
    }
    let name = std::iter::once(first).chain(others.iter().map(String::as_str)).collect::<Vec<_>>().join(", ");
    Ok(MatchGroup { name, members, tolerance, match_tolerance })
}

/// The members one target of a bench matched group stands for.
fn group_members(target: &str, location: &Location, lookup: &Lookup) -> Result<Vec<Target>> {
    if lookup.params.contains_key(target) {
        return Err(error(
            "SIM091",
            format!("{} is a param; a matched group holds component values and model parameters", repr(target)),
            location,
        ));
    }
    if let Some(component) = lookup.components.iter().find(|component| component.reference == target) {
        return match &component.treatment {
            Treatment::Passive { .. } => {
                if lookup.instance(target).and_then(group_name).is_some() {
                    return Err(error(
                        "SIM091",
                        format!(
                            "{} is in a matched group on its component; change the tolerances of that group there",
                            repr(target)
                        ),
                        location,
                    ));
                }
                Ok(vec![Target::Component(target.to_owned())])
            }
            Treatment::PartModel => {
                let units = load_resistances(component, lookup);
                if units.is_empty() {
                    return Err(error(
                        "SIM091",
                        format!(
                            "{} is no passive and has no load resistance; name a model parameter, {target}.PARAMETER",
                            repr(target)
                        ),
                        location,
                    ));
                }
                Ok(units)
            }
            _ => Err(error(
                "SIM091",
                format!("{} is not simulated as a passive or a part model in this bench", repr(target)),
                location,
            )),
        };
    }
    let Some((instance, parameter)) = target.rsplit_once('.') else {
        let names: Vec<&str> = lookup.components.iter().map(|component| component.reference.as_str()).collect();
        return Err(suggesting(
            error(
                "SIM091",
                format!("tolerance names {}, which is no component or COMPONENT.PARAMETER", repr(target)),
                location,
            ),
            target,
            &names,
        ));
    };
    // One unit of a multi-unit part: COMPONENT.UNIT.PARAMETER.
    let (reference, unit) = match instance.split_once('.') {
        Some((reference, unit)) => (reference, Some(unit)),
        None => (instance, None),
    };
    let specs: Vec<(String, &SimSpec)> =
        if let Some(component) = lookup.components.iter().find(|component| component.reference == reference) {
            match &component.treatment {
                Treatment::PartModel => unit_specs(component, lookup)
                    .into_iter()
                    .filter(|(name, _)| unit.is_none_or(|unit| name == &format!("{reference}.{unit}")))
                    .collect(),
                _ => lookup
                    .specs(reference)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|spec| (reference.to_owned(), spec))
                    .collect(),
            }
        } else {
            lookup.specs(reference).unwrap_or_default().into_iter().map(|spec| (reference.to_owned(), spec)).collect()
        };
    if specs.is_empty() {
        return Err(error(
            "SIM091",
            format!(
                "tolerance {target}: {} is no component with a sim model, unit of one, replacement or bench model",
                repr(instance)
            ),
            location,
        ));
    }
    let members: Vec<Target> = specs
        .iter()
        .filter(|(_, spec)| spec.parameters.contains_key(parameter))
        .map(|(name, _)| Target::ModelParameter { instance: name.clone(), parameter: parameter.to_owned() })
        .collect();
    if members.is_empty() {
        let mut keys: Vec<&str> = Vec::new();
        for (_, spec) in &specs {
            keys.extend(spec.parameters.keys().map(String::as_str));
        }
        return Err(suggesting(
            error(
                "SIM091",
                format!(
                    "tolerance {target}: the model of {instance} does not declare {}, so it has no nominal value to vary",
                    repr(parameter)
                ),
                location,
            ),
            parameter,
            &keys,
        ));
    }
    Ok(members)
}

fn monte_carlo(item: &VariationDecl) -> Result<Method> {
    let mut runs = None;
    let mut seed = 1u64;
    let mut distribution = Distribution::Uniform;
    for entry in &item.entries {
        let SimValue::Scalar(value) = &entry.value else {
            return Err(error("SIM090", format!("montecarlo {} takes one value", entry.key), &entry.location));
        };
        match entry.key.as_str() {
            "runs" => match value {
                Scalar::Int(number) if *number >= 1.into() && *number <= MAX_RUNS.into() => {
                    runs = u32::try_from(number.clone()).ok();
                }
                _ => {
                    return Err(error(
                        "SIM090",
                        format!("montecarlo runs must be a whole number from 1 to {MAX_RUNS}"),
                        &entry.location,
                    ));
                }
            },
            "seed" => match value {
                Scalar::Int(number) if *number >= 0.into() => match u64::try_from(number.clone()) {
                    Ok(number) => seed = number,
                    Err(_) => {
                        return Err(error("SIM090", "montecarlo seed must fit in 64 bits", &entry.location));
                    }
                },
                _ => return Err(error("SIM090", "montecarlo seed must be a whole number, 0 or more", &entry.location)),
            },
            "distribution" => {
                distribution = match value {
                    Scalar::Str(name) if name == "uniform" => Distribution::Uniform,
                    Scalar::Str(name) if name == "gaussian" => Distribution::Gaussian,
                    _ => {
                        return Err(error(
                            "SIM090",
                            "montecarlo distribution must be uniform or gaussian",
                            &entry.location,
                        ));
                    }
                }
            }
            other => {
                return Err(suggesting(
                    error("SIM090", format!("unknown montecarlo property {}", repr(other)), &entry.location),
                    other,
                    &["runs", "seed", "distribution"],
                ));
            }
        }
    }
    let Some(runs) = runs else {
        return Err(error("SIM090", "montecarlo needs runs: montecarlo { runs = 200; seed = 1; }", &item.location));
    };
    Ok(Method::MonteCarlo { runs, seed, distribution })
}

/// A `tolerance TARGET = value;` statement.
fn bench_tolerance(target: &str, value: &Scalar, location: &Location, lookup: &Lookup) -> Result<Tolerance> {
    let made =
        |target: Target, spread: Spread| Tolerance { target, spread, declared_by: "bench", location: location.clone() };
    if let Some(param) = lookup.params.get(target) {
        if param.derived.is_some() {
            return Err(error(
                "SIM091",
                format!(
                    "param {} is derived from other params; give the params it is computed from a tolerance",
                    repr(target)
                ),
                location,
            ));
        }
        let label = format!("tolerance {target}");
        return Ok(made(Target::Param(target.to_owned()), spread(value, param.value.dimension(), &label, location)?));
    }
    if let Some((instance, parameter)) = target.split_once('.') {
        let Some(specs) = lookup.specs(instance) else {
            let names: Vec<&str> = lookup
                .components
                .iter()
                .filter(|c| matches!(c.treatment, Treatment::PartModel | Treatment::Replaced(_)))
                .map(|c| c.reference.as_str())
                .chain(lookup.models.iter().map(|m| m.name.as_str()))
                .collect();
            return Err(suggesting(
                error(
                    "SIM091",
                    format!(
                        "tolerance {target}: {} is no component with a sim model, replacement or bench model",
                        repr(instance)
                    ),
                    location,
                ),
                instance,
                &names,
            ));
        };
        let Some((spec, dimension)) =
            specs.iter().find_map(|spec| parameter_dimension(spec, parameter).map(|dimension| (*spec, dimension)))
        else {
            let mut keys: Vec<&str> = Vec::new();
            for spec in &specs {
                keys.extend(spec.kind.parameters().iter().map(|(name, _)| *name));
            }
            return Err(suggesting(
                error(
                    "SIM091",
                    format!("tolerance {target}: the model of {instance} has no parameter {}", repr(parameter)),
                    location,
                ),
                parameter,
                &keys,
            ));
        };
        let label = format!("tolerance {target}");
        let spread = spread(value, dimension, &label, location)?;
        let declared = specs.iter().any(|spec| spec.parameters.contains_key(parameter));
        if !declared && !(matches!(spread, Spread::Absolute(_)) && ZERO_DEFAULT.contains(&parameter)) {
            return Err(error(
                "SIM091",
                format!(
                    "tolerance {target}: the {} model of {instance} does not declare {parameter}, so it has no nominal value to vary",
                    spec.kind.name()
                ),
                location,
            ));
        }
        return Ok(made(
            Target::ModelParameter { instance: instance.to_owned(), parameter: parameter.to_owned() },
            spread,
        ));
    }
    if let Some(component) = lookup.components.iter().find(|component| component.reference == target) {
        let Treatment::Passive { kind, .. } = &component.treatment else {
            return Err(error(
                "SIM091",
                format!(
                    "tolerance {target}: {} is not simulated as a resistor, capacitor or inductor; vary a model parameter instead ({target}.PARAMETER)",
                    repr(target)
                ),
                location,
            ));
        };
        let label = format!("tolerance {target}");
        return Ok(made(Target::Component(target.to_owned()), spread(value, kind.dimension(), &label, location)?));
    }
    let mut names: Vec<&str> = lookup.params.keys().map(String::as_str).collect();
    names.extend(lookup.components.iter().map(|component| component.reference.as_str()));
    Err(suggesting(
        error(
            "SIM091",
            format!("tolerance names {}, which is no param, component or COMPONENT.PARAMETER", repr(target)),
            location,
        ),
        target,
        &names,
    ))
}

/// The variation of a bench as JSON.
pub fn variation_value(variation: &Variation) -> Value {
    let text = |value: &str| Value::from(value);
    let mut method = vec![("kind".to_owned(), text(variation.method.name()))];
    if let Method::MonteCarlo { runs, seed, distribution } = &variation.method {
        method.push(("runs".to_owned(), Value::from(i64::from(*runs))));
        // Seeds above i64::MAX are written as text.
        method.push((
            "seed".to_owned(),
            i64::try_from(*seed).map(Value::from).unwrap_or_else(|_| Value::from(seed.to_string().as_str())),
        ));
        method.push(("distribution".to_owned(), text(distribution.name())));
    }
    let tolerances = variation
        .tolerances
        .iter()
        .map(|item| {
            Value::Object(
                vec![
                    ("target".to_owned(), text(&item.target.text())),
                    ("kind".to_owned(), text(item.target.kind())),
                    ("relative".to_owned(), Value::Bool(matches!(item.spread, Spread::Relative(_)))),
                    ("tolerance".to_owned(), quantity_value(item.spread.quantity())),
                    ("declared_by".to_owned(), text(item.declared_by)),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    let groups = variation
        .groups
        .iter()
        .map(|group| {
            Value::Object(
                vec![
                    ("name".to_owned(), text(&group.name)),
                    ("members".to_owned(), Value::Array(group.members.iter().map(|m| text(&m.text())).collect())),
                    ("tolerance".to_owned(), quantity_value(&group.tolerance)),
                    ("match_tolerance".to_owned(), quantity_value(&group.match_tolerance)),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    Value::Object(
        vec![
            ("method".to_owned(), Value::Object(method.into_iter().collect())),
            ("tolerances".to_owned(), Value::Array(tolerances)),
            ("groups".to_owned(), Value::Array(groups)),
        ]
        .into_iter()
        .collect(),
    )
}

//! Stability analyses of benches (`docs/language-reference.md`, "Stability
//! analysis"): the loop gain at a probe and the poles and zeros of the
//! linearised circuit.
//!
//! ```copper
//! analysis loop { from = 1kHz; to = 1GHz; points_per_decade = 20; probe = U1.OUT; }
//! measure pm = phase_margin(loop_gain()) { min = 45deg; }
//!
//! analysis pz;                                     // poles only
//! analysis pz { input = J_IN; output = V(OUT); }   // poles, and the zeros of V(OUT) / J_IN
//! ```
//!
//! The probe is a component pin: the simulator opens the connection between
//! that pin and its net and inserts its 0 V probe there, so the loop gain is
//! that of every loop through the pin (`SIM098`). A pole-zero analysis has
//! the built-in measure `stable` (`SIM100`).

use copper_json::Value;
use copper_json::python_str_repr as repr;
use copper_units::Dimension;

use crate::bench::{Analysis, BenchModel, Replacement};
use crate::error::CopperScriptError;
use crate::expression::Node;
use crate::model::Endpoint;
use crate::noise::NoiseInput;
use crate::simulation::{self, SimQuantity, ValueScope};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

fn unknown(kind: &str, key: &str, keys: &[&str], location: &Location) -> CopperScriptError {
    let mut failure = error("SIM040", format!("unknown analysis {kind} property {}", repr(key)), location);
    if let Some(suggestion) = close_match(key, keys) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    failure
}

/// `analysis loop { from; to; points_per_decade; probe = COMPONENT.PIN; }`.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopAnalysis {
    pub from: SimQuantity,
    pub to: SimQuantity,
    pub points_per_decade: u32,
    /// The pin the probe is inserted at, between it and its net.
    pub probe: Endpoint,
    /// The node of the net on the other side of the probe.
    pub net: Node,
}

/// `analysis pz;` or `analysis pz { input = SOURCE; output = V(OUT); }`.
#[derive(Clone, Debug, PartialEq)]
pub struct PzAnalysis {
    /// The source the zeros' transfer function is per unit of.
    pub input: Option<NoiseInput>,
    /// The transfer function's output and its reference (`None` is ground).
    pub output: Option<(Node, Option<Node>)>,
}

/// Lower `analysis loop { ... }`. `probe` checks the probe pin and returns
/// the node of its net.
pub(crate) fn loop_analysis(
    item: &AnalysisDecl,
    scope: &ValueScope,
    probe: &dyn Fn(&Endpoint, &Location) -> Result<Node>,
) -> Result<Analysis> {
    let mut from = None;
    let mut to = None;
    let mut points = None;
    let mut pin = None;
    for entry in &item.entries {
        let SimValue::Scalar(value) = &entry.value else {
            return Err(error("SIM098", format!("analysis loop {} takes one value", entry.key), &entry.location));
        };
        match entry.key.as_str() {
            "from" | "to" => {
                let label = format!("analysis loop {}", entry.key);
                let value = simulation::typed_value(value, Dimension::Frequency, &label, scope, &entry.location)?;
                if entry.key == "from" {
                    from = Some(value);
                } else {
                    to = Some(value);
                }
            }
            "points_per_decade" => match value {
                Scalar::Int(number) if *number > 0.into() && *number <= 10000.into() => {
                    points = u32::try_from(number.clone()).ok();
                }
                _ => {
                    return Err(error(
                        "SIM040",
                        "analysis loop points_per_decade must be a whole number from 1 to 10000",
                        &entry.location,
                    ));
                }
            },
            "probe" => {
                let Scalar::Str(target) = value else {
                    return Err(error(
                        "SIM098",
                        "analysis loop probe names a component pin: probe = U1.OUT;",
                        &entry.location,
                    ));
                };
                let Ok(endpoint) = Endpoint::parse(target) else {
                    return Err(error(
                        "SIM098",
                        format!(
                            "analysis loop probe {} is no component pin; the probe goes in series with a pin: probe = U1.OUT;",
                            repr(target)
                        ),
                        &entry.location,
                    ));
                };
                let net = probe(&endpoint, &entry.location)?;
                pin = Some((endpoint, net));
            }
            other => {
                return Err(unknown("loop", other, &["from", "to", "points_per_decade", "probe"], &entry.location));
            }
        }
    }
    let (Some(from), Some(to), Some(points_per_decade)) = (from, to, points) else {
        return Err(error("SIM040", "analysis loop needs from, to and points_per_decade", &item.location));
    };
    if from.value.base_value().is_zero()
        || from.value.base_value().is_negative()
        || to.value.base_value() <= from.value.base_value()
    {
        return Err(error(
            "SIM040",
            format!("analysis loop needs 0 < from < to, found from {} to {}", from.value, to.value),
            &item.location,
        ));
    }
    let Some((probe, net)) = pin else {
        return Err(error(
            "SIM098",
            "analysis loop needs its probe: the component pin where the loop is read, probe = U1.OUT;",
            &item.location,
        ));
    };
    Ok(Analysis::Loop(Box::new(LoopAnalysis { from, to, points_per_decade, probe, net })))
}

/// Lower `analysis pz { ... }`. `node` resolves a node as `V(...)` does.
pub(crate) fn pz_analysis(
    item: &AnalysisDecl,
    node: &dyn Fn(&str, &Location) -> Result<Node>,
    replacements: &[Replacement],
    models: &[BenchModel],
) -> Result<Analysis> {
    let mut input = None;
    let mut output = None;
    for entry in &item.entries {
        match entry.key.as_str() {
            "output" => {
                let SimValue::List(nodes) = &entry.value else {
                    return Err(error(
                        "SIM100",
                        "analysis pz output is one voltage probe: output = V(OUT); or output = V(P, N);",
                        &entry.location,
                    ));
                };
                let mut resolved = Vec::new();
                for value in nodes {
                    let Scalar::Str(name) = value else {
                        return Err(error("SIM100", "analysis pz output names nodes", &entry.location));
                    };
                    resolved.push(node(name, &entry.location)?);
                }
                if resolved.len() > 2 {
                    return Err(error("SIM100", "output = V(P, N) takes at most two nodes", &entry.location));
                }
                let mut resolved = resolved.into_iter();
                output = Some((resolved.next().expect("the parser reads one node"), resolved.next()));
            }
            "input" => match &entry.value {
                SimValue::Scalar(Scalar::Str(name)) => {
                    input = Some(pz_input(name, replacements, models, &entry.location)?);
                }
                _ => return Err(error("SIM100", "analysis pz input names a source", &entry.location)),
            },
            other => return Err(unknown("pz", other, &["input", "output"], &entry.location)),
        }
    }
    if input.is_some() != output.is_some() {
        return Err(error(
            "SIM100",
            "analysis pz takes input and output together (the zeros of V(output) per unit of input), or neither",
            &item.location,
        ));
    }
    Ok(Analysis::Pz(Box::new(PzAnalysis { input, output })))
}

/// The source `name` names: a bench `model` of kind `source`, or a component
/// or supply replaced with one.
fn pz_input(
    name: &str,
    replacements: &[Replacement],
    models: &[BenchModel],
    location: &Location,
) -> Result<NoiseInput> {
    let dimension = |spec: &simulation::SimSpec| spec.output.unwrap_or(simulation::SourceOutput::Voltage).dimension();
    let source = |kind: simulation::SimKind, what: &str| {
        if kind == simulation::SimKind::Source {
            Ok(())
        } else {
            Err(error(
                "SIM100",
                format!("analysis pz input {} is {what} {} model; the input must be a source", repr(name), kind.name()),
                location,
            ))
        }
    };
    if let Some(model) = models.iter().find(|model| model.name == name) {
        source(model.spec.kind, "a")?;
        return Ok(NoiseInput { name: name.to_owned(), instance: name.to_owned(), dimension: dimension(&model.spec) });
    }
    if let Some(replacement) = replacements.iter().find(|replacement| replacement.target == name) {
        source(replacement.spec.kind, "replaced with a")?;
        let instance = if replacement.supply { format!("supply {name}") } else { name.to_owned() };
        return Ok(NoiseInput { name: name.to_owned(), instance, dimension: dimension(&replacement.spec) });
    }
    let mut names: Vec<&str> = models.iter().map(|model| model.name.as_str()).collect();
    names.extend(replacements.iter().map(|replacement| replacement.target.as_str()));
    let mut failure = error(
        "SIM100",
        format!(
            "analysis pz input {} is no source of the bench; name a bench model or replacement of kind source",
            repr(name)
        ),
        location,
    );
    if let Some(suggestion) = close_match(name, &names) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    Err(failure)
}

/// The analysis as JSON (`kind` `loop`).
pub fn loop_value(
    analysis: &LoopAnalysis,
    node: &dyn Fn(&Node) -> Value,
    quantity: &dyn Fn(&SimQuantity) -> Value,
) -> Value {
    Value::Object(
        vec![
            ("kind".to_owned(), Value::from("loop")),
            ("from".to_owned(), quantity(&analysis.from)),
            ("to".to_owned(), quantity(&analysis.to)),
            ("points_per_decade".to_owned(), Value::from(i64::from(analysis.points_per_decade))),
            ("probe".to_owned(), Value::from(analysis.probe.to_string().as_str())),
            ("net".to_owned(), node(&analysis.net)),
        ]
        .into_iter()
        .collect(),
    )
}

/// The analysis as JSON (`kind` `pz`).
pub fn pz_value(analysis: &PzAnalysis, node: &dyn Fn(&Node) -> Value) -> Value {
    let (output, reference) = match &analysis.output {
        Some((output, reference)) => (node(output), reference.as_ref().map(node).unwrap_or(Value::Null)),
        None => (Value::Null, Value::Null),
    };
    Value::Object(
        vec![
            ("kind".to_owned(), Value::from("pz")),
            ("input".to_owned(), analysis.input.as_ref().map(|i| Value::from(i.name.as_str())).unwrap_or(Value::Null)),
            ("output".to_owned(), output),
            ("reference".to_owned(), reference),
        ]
        .into_iter()
        .collect(),
    )
}

/// The name of the built-in measure of `analysis pz`.
pub const STABLE_MEASURE: &str = "stable";

/// `measure stable = stable() { min = 1; }`: added to every pole-zero analysis.
pub(crate) fn stable_measure(location: &Location) -> crate::bench::Measure {
    use crate::expression::{Expression, ExpressionKind, ExpressionType, Function, Units};
    let one = copper_units::Quantity::from_base(Dimension::Ratio, copper_units::Decimal::parse("1").expect("one"), "%")
        .expect("% is the ratio unit");
    crate::bench::Measure {
        name: STABLE_MEASURE.to_owned(),
        expression: Expression {
            location: location.clone(),
            kind: ExpressionKind::Call { function: Function::Stable, arguments: Vec::new() },
            ty: ExpressionType::scalar(Units::DIMENSIONLESS),
        },
        min: Some(SimQuantity { value: one, param: None }),
        max: None,
        conditions: Vec::new(),
        location: location.clone(),
    }
}

/// The plot of an `analysis loop` section that plots nothing: the Bode plot
/// of its loop gain, `db(loop_gain())` above `phase(loop_gain())`.
pub(crate) fn default_loop_plot(location: &Location) -> crate::bench::Plot {
    use crate::expression::{Expression, ExpressionKind, ExpressionType, Function, Probe, Units};
    let gain = Expression {
        location: location.clone(),
        kind: ExpressionKind::Probe(Probe::LoopGain),
        ty: ExpressionType { units: Units::DIMENSIONLESS, complex: true, swept: true },
    };
    let of = |function: Function, dimension: Dimension| Expression {
        location: location.clone(),
        kind: ExpressionKind::Call { function, arguments: vec![gain.clone()] },
        ty: ExpressionType { units: Units::of(dimension), complex: false, swept: true },
    };
    crate::bench::Plot {
        expressions: vec![of(Function::Db, Dimension::Gain), of(Function::Phase, Dimension::Angle)],
        location: location.clone(),
    }
}

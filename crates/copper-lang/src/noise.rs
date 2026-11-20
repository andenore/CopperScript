//! `analysis noise { ... }` of benches (`docs/language-reference.md`,
//! "Noise analysis"): the sweep, the output probe, the optional input source
//! and the resistor temperature.
//!
//! ```copper
//! analysis noise {
//!     from = 10Hz; to = 100kHz; points_per_decade = 20;
//!     output = V(OUT);            // or V(P, N), or several: V(A), V(B)
//!     input = SRC;                // a source model: bench model or replacement
//!     temperature = 27degC;       // or 300.15K; 27 degC by default
//! }
//! ```

use copper_json::Value;
use copper_json::python_str_repr as repr;
use copper_units::{Decimal, Dimension};

use crate::bench::{Analysis, BenchModel, Replacement};
use crate::error::CopperScriptError;
use crate::expression::{Node, NoiseScope};
use crate::simulation::{self, SimKind, SimQuantity, SourceOutput, ValueScope};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

/// The noise temperature as written (`27 degC`) and in kelvin.
#[derive(Clone, Debug, PartialEq)]
pub struct Temperature {
    pub written: String,
    pub kelvin: Decimal,
}

/// 0 degrees Celsius in kelvin.
const ZERO_CELSIUS: &str = "273.15";

/// The source a noise analysis refers its output to.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseInput {
    /// The name as written: a bench model, a replaced component or a replaced supply.
    pub name: String,
    /// The simulator instance of that source (`supply NAME` for a supply).
    pub instance: String,
    /// `Voltage` or `Current`: what the source sets.
    pub dimension: Dimension,
}

/// The parts of `Analysis::Noise` beyond the sweep.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseAnalysis {
    pub from: SimQuantity,
    pub to: SimQuantity,
    pub points_per_decade: u32,
    /// The (first) output node, and its reference (`None` is the ground).
    pub output: Node,
    pub reference: Option<Node>,
    /// Every output, the first included: `output = V(A), V(B, C);`.
    pub outputs: Vec<(Node, Option<Node>)>,
    pub input: Option<NoiseInput>,
    /// `None` is the default, 27 degC.
    pub temperature: Option<Temperature>,
}

impl NoiseAnalysis {
    /// What the expression checker needs to know.
    pub fn scope(&self) -> NoiseScope {
        NoiseScope { input: self.input.as_ref().map(|input| input.dimension), outputs: self.outputs.len() }
    }

    /// The temperature in kelvin.
    pub fn kelvin(&self) -> f64 {
        match &self.temperature {
            Some(t) => t.kelvin.to_sci_string().parse().unwrap_or(f64::NAN),
            None => 300.15,
        }
    }
}

fn temperature(value: &Scalar, location: &Location) -> Result<Temperature> {
    let invalid =
        || error("SIM094", "analysis noise temperature must be in degC or K, such as 27degC or 300K", location);
    let Scalar::Quantity { value, unit } = value else { return Err(invalid()) };
    let number = Decimal::parse(value).map_err(|_| invalid())?;
    let kelvin = match unit.as_str() {
        "degC" => number.add(&Decimal::parse(ZERO_CELSIUS).map_err(|_| invalid())?).map_err(|_| invalid())?,
        "K" => number,
        _ => return Err(invalid()),
    };
    if kelvin.is_zero() || kelvin.is_negative() {
        return Err(error(
            "SIM094",
            format!("analysis noise temperature {value} {unit} is not above absolute zero"),
            location,
        ));
    }
    Ok(Temperature {
        written: format!("{} {unit}", Decimal::parse(value).map_err(|_| invalid())?.decimal_text()),
        kelvin,
    })
}

/// The source `name` names: a bench `model` of kind `source`, or a component or supply replaced with one.
fn input(name: &str, replacements: &[Replacement], models: &[BenchModel], location: &Location) -> Result<NoiseInput> {
    let dimension = |spec: &simulation::SimSpec| spec.output.unwrap_or(SourceOutput::Voltage).dimension();
    if let Some(model) = models.iter().find(|model| model.name == name) {
        if model.spec.kind != SimKind::Source {
            return Err(error(
                "SIM094",
                format!("noise input {} is a {} model; the input must be a source", repr(name), model.spec.kind.name()),
                location,
            ));
        }
        return Ok(NoiseInput { name: name.to_owned(), instance: name.to_owned(), dimension: dimension(&model.spec) });
    }
    if let Some(replacement) = replacements.iter().find(|replacement| replacement.target == name) {
        if replacement.spec.kind != SimKind::Source {
            return Err(error(
                "SIM094",
                format!(
                    "noise input {} is replaced with a {} model; the input must be a source",
                    repr(name),
                    replacement.spec.kind.name()
                ),
                location,
            ));
        }
        let instance = if replacement.supply { format!("supply {name}") } else { name.to_owned() };
        return Ok(NoiseInput { name: name.to_owned(), instance, dimension: dimension(&replacement.spec) });
    }
    let mut names: Vec<&str> = models.iter().map(|model| model.name.as_str()).collect();
    names.extend(replacements.iter().map(|replacement| replacement.target.as_str()));
    let mut failure = error(
        "SIM094",
        format!(
            "noise input {} is no source of the bench; name a bench model or replacement of kind source",
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

/// Lower `analysis noise { ... }`. `node` resolves a node name as `V(...)` does.
pub(crate) fn analysis(
    item: &AnalysisDecl,
    scope: &ValueScope,
    node: &dyn Fn(&str, &Location) -> Result<Node>,
    replacements: &[Replacement],
    models: &[BenchModel],
) -> Result<Analysis> {
    let mut from = None;
    let mut to = None;
    let mut points = None;
    let mut output = None;
    let mut source = None;
    let mut temp = None;
    for entry in &item.entries {
        let single = || match &entry.value {
            SimValue::Scalar(value) => Ok(value),
            _ => Err(error("SIM040", format!("analysis noise {} takes one value", entry.key), &entry.location)),
        };
        match entry.key.as_str() {
            "from" | "to" => {
                let label = format!("analysis noise {}", entry.key);
                let value = simulation::typed_value(single()?, Dimension::Frequency, &label, scope, &entry.location)?;
                if entry.key == "from" {
                    from = Some(value);
                } else {
                    to = Some(value);
                }
            }
            "points_per_decade" => match single()? {
                Scalar::Int(number) if *number > 0.into() && *number <= 10000.into() => {
                    points = u32::try_from(number.clone()).ok();
                }
                _ => {
                    return Err(error(
                        "SIM040",
                        "analysis noise points_per_decade must be a whole number from 1 to 10000",
                        &entry.location,
                    ));
                }
            },
            "output" => {
                let probes: Vec<(&SimValue, &Location)> = match &entry.value {
                    SimValue::Block { kind, entries } if kind == "outputs" => {
                        entries.iter().map(|probe| (&probe.value, &probe.location)).collect()
                    }
                    value => vec![(value, &entry.location)],
                };
                let mut all: Vec<(Node, Option<Node>)> = Vec::new();
                for (value, location) in probes {
                    let SimValue::List(nodes) = value else {
                        return Err(error(
                            "SIM094",
                            "analysis noise output is a voltage probe: output = V(OUT); or output = V(P, N); or several, output = V(A), V(B);",
                            location,
                        ));
                    };
                    let mut resolved = Vec::new();
                    for value in nodes {
                        let Scalar::Str(name) = value else {
                            return Err(error("SIM094", "analysis noise output names nodes", location));
                        };
                        resolved.push(node(name, location)?);
                    }
                    if resolved.len() > 2 {
                        return Err(error("SIM094", "output = V(P, N) takes at most two nodes", location));
                    }
                    let mut resolved = resolved.into_iter();
                    let probe = (resolved.next().expect("the parser reads one node"), resolved.next());
                    if all.contains(&probe) {
                        return Err(error("SIM094", "analysis noise names one output twice", location));
                    }
                    all.push(probe);
                }
                output = Some(all);
            }
            "input" => match single()? {
                Scalar::Str(name) => source = Some(input(name, replacements, models, &entry.location)?),
                _ => return Err(error("SIM094", "analysis noise input names a source", &entry.location)),
            },
            "temperature" => temp = Some(temperature(single()?, &entry.location)?),
            other => {
                let keys = ["from", "to", "points_per_decade", "output", "input", "temperature"];
                let mut failure =
                    error("SIM040", format!("unknown analysis noise property {}", repr(other)), &entry.location);
                if let Some(suggestion) = close_match(other, &keys) {
                    failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
                    failure.replacement = Some(suggestion);
                }
                return Err(failure);
            }
        }
    }
    let (Some(from), Some(to), Some(points_per_decade)) = (from, to, points) else {
        return Err(error("SIM040", "analysis noise needs from, to and points_per_decade", &item.location));
    };
    if from.value.base_value().is_zero()
        || from.value.base_value().is_negative()
        || to.value.base_value() <= from.value.base_value()
    {
        return Err(error(
            "SIM040",
            format!("analysis noise needs 0 < from < to, found from {} to {}", from.value, to.value),
            &item.location,
        ));
    }
    let Some(outputs) = output else {
        return Err(error("SIM094", "analysis noise needs its output: output = V(OUT);", &item.location));
    };
    let (output, reference) = outputs[0].clone();
    Ok(Analysis::Noise(Box::new(NoiseAnalysis {
        from,
        to,
        points_per_decade,
        output,
        reference,
        outputs,
        input: source,
        temperature: temp,
    })))
}

/// A `noise(X)` contributor: a simulated (not ignored) component, or a bench model.
pub(crate) fn contributor(target: &str, names: &[String], location: &Location) -> Result<String> {
    if names.iter().any(|name| name == target) {
        return Ok(target.to_owned());
    }
    let candidates: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut failure = error(
        "SIM087",
        format!("noise({target}): {} is no simulated component or bench model", repr(target)),
        location,
    );
    if let Some(suggestion) = close_match(target, &candidates) {
        failure.hint = Some(format!("did you mean {}?", repr(&suggestion)));
        failure.replacement = Some(suggestion);
    }
    Err(failure)
}

/// The analysis as JSON (`kind` `noise`), with the bench's own node and quantity values.
pub fn analysis_value(
    noise: &NoiseAnalysis,
    node: &dyn Fn(&Node) -> Value,
    quantity: &dyn Fn(&SimQuantity) -> Value,
) -> Value {
    let input = noise.input.as_ref().map(|input| {
        Value::Object(
            vec![
                ("name".to_owned(), Value::from(input.name.as_str())),
                ("dimension".to_owned(), Value::from(input.dimension.name())),
            ]
            .into_iter()
            .collect(),
        )
    });
    let mut entries = vec![
        ("kind".to_owned(), Value::from("noise")),
        ("from".to_owned(), quantity(&noise.from)),
        ("to".to_owned(), quantity(&noise.to)),
        ("points_per_decade".to_owned(), Value::from(i64::from(noise.points_per_decade))),
        ("output".to_owned(), node(&noise.output)),
        ("reference".to_owned(), noise.reference.as_ref().map(node).unwrap_or(Value::Null)),
        ("input".to_owned(), input.unwrap_or(Value::Null)),
        ("temperature_k".to_owned(), Value::Float(noise.kelvin())),
    ];
    if let Some(t) = &noise.temperature {
        entries.push(("temperature".to_owned(), Value::from(t.written.as_str())));
    }
    if noise.outputs.len() > 1 {
        let outputs = noise
            .outputs
            .iter()
            .map(|(output, reference)| {
                Value::Object(
                    vec![
                        ("output".to_owned(), node(output)),
                        ("reference".to_owned(), reference.as_ref().map(node).unwrap_or(Value::Null)),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect();
        entries.push(("outputs".to_owned(), Value::Array(outputs)));
    }
    Value::Object(entries.into_iter().collect())
}

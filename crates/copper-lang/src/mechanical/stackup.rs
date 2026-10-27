//! A declared board stack-up (`pcbir.mechanical_stackup`): copper and
//! dielectric layers from top to bottom.

use copper_json::python_str_repr as repr;
use copper_physical::CopperLayer;
use copper_units::{Decimal, Dimension, Quantity};

use crate::error::CopperScriptError;
use crate::pyvalue;
use crate::syntax::{Location, Scalar, StackupDecl, StackupLayerDecl};

/// `StackupLayer` as declared: copper layers name their `CopperLayer`.
#[derive(Clone, Debug, PartialEq)]
pub struct StackupLayer {
    pub id: String,
    pub copper_layer: Option<CopperLayer>,
    pub thickness_nm: i64,
    pub material: Option<String>,
    pub relative_permittivity: Option<Decimal>,
    pub loss_tangent: Option<Decimal>,
    pub dielectric_type: Option<String>,
}

/// `MechanicalStackup`: validated top-to-bottom physical layers.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalStackup {
    pub layers: Vec<StackupLayer>,
    pub location: Location,
}

type Result<T> = std::result::Result<T, CopperScriptError>;

fn error<T>(message: String, location: &Location) -> Result<T> {
    Err(CopperScriptError::new("MEC006", message, location.clone()))
}

/// `_length_nm`: a positive exact length, or the reason it is not.
fn length_nm(value: &Scalar) -> std::result::Result<i64, String> {
    let Scalar::Quantity { value, unit } = value else { return Err("must be a typed length".to_owned()) };
    let number = Decimal::parse(value).map_err(|failure| failure.to_string())?;
    let quantity =
        Quantity::of(Dimension::Length, &number, unit).map_err(|failure| format!("must be a length ({failure})"))?;
    let nm = quantity.base_value().mul(&Decimal::from_i64(1_000_000_000)).map_err(|failure| failure.to_string())?;
    if !nm.is_integral() {
        return Err("must be an exact integer number of nanometres".to_owned());
    }
    if nm <= Decimal::zero() {
        return Err("must be positive".to_owned());
    }
    nm.to_i64().filter(|nm| *nm <= copper_physical::MAX_COORDINATE).ok_or_else(|| "must be positive".to_owned())
}

fn positive_number(value: &Scalar, name: &str, item: &StackupLayerDecl) -> Result<Decimal> {
    let number = match value {
        Scalar::Int(_) | Scalar::Float(_) => pyvalue::decimal(value),
        _ => {
            return error(
                format!("dielectric layer {} {name} must be a unitless number", repr(&item.name)),
                &item.location,
            );
        }
    };
    match number {
        Some(number) if number > Decimal::zero() => Ok(number),
        _ => error(format!("dielectric layer {} {name} must be positive", repr(&item.name)), &item.location),
    }
}

/// `lower_stackup`: validate one `stackup` block.
pub fn lower_stackup(declaration: &StackupDecl) -> Result<MechanicalStackup> {
    if declaration.layers.is_empty() {
        return error("stackup requires at least F.Cu and B.Cu copper layers".to_owned(), &declaration.location);
    }
    let mut names: Vec<&str> = Vec::new();
    let mut layers: Vec<StackupLayer> = Vec::new();
    for (index, item) in declaration.layers.iter().enumerate() {
        let fail = |message: String| error::<StackupLayer>(message, &item.location);
        let expected = if index % 2 == 0 { "copper" } else { "dielectric" };
        if item.kind != expected {
            fail(format!(
                "stackup layers must alternate copper and dielectric from top to bottom; expected {expected}, found {} {}",
                item.kind,
                repr(&item.name)
            ))?;
        }
        if names.contains(&item.name.as_str()) {
            fail(format!("duplicate stackup layer {}", repr(&item.name)))?;
        }
        names.push(&item.name);
        let copper = item.kind == "copper";
        let allowed: &[&str] =
            if copper { &["thickness"] } else { &["thickness", "er", "loss_tangent", "material", "type"] };
        if let Some(unknown) = item.parameters.keys().filter(|key| !allowed.contains(&key.as_str())).min() {
            fail(format!("unknown {} stackup property {}", item.kind, repr(unknown)))?;
        }
        let required: &[&str] = if copper { &["thickness"] } else { &["er", "thickness"] };
        if let Some(missing) = required.iter().find(|key| !item.parameters.contains_key(**key)) {
            fail(format!("{} layer {} requires {}", item.kind, repr(&item.name), repr(missing)))?;
        }
        let thickness = match length_nm(&item.parameters["thickness"]) {
            Ok(thickness) => thickness,
            Err(reason) => {
                return error(format!("{} layer {} thickness {reason}", item.kind, repr(&item.name)), &item.location);
            }
        };
        if copper {
            let Some(layer) = CopperLayer::parse(&item.name) else {
                return error(
                    format!("unknown copper layer {}; expected F.Cu, In1.Cu ... or B.Cu", repr(&item.name)),
                    &item.location,
                );
            };
            layers.push(StackupLayer {
                id: item.name.clone(),
                copper_layer: Some(layer),
                thickness_nm: thickness,
                material: None,
                relative_permittivity: None,
                loss_tangent: None,
                dielectric_type: None,
            });
            continue;
        }
        let er = positive_number(&item.parameters["er"], "er", item)?;
        if er < Decimal::from_i64(1) {
            fail(format!("dielectric layer {} er must be at least 1 (relative permittivity)", repr(&item.name)))?;
        }
        let loss = item
            .parameters
            .get("loss_tangent")
            .map(|value| positive_number(value, "loss_tangent", item))
            .transpose()?;
        let material = match item.parameters.get("material") {
            None => None,
            Some(Scalar::Str(text)) if !super::strip(text).is_empty() => Some(text.clone()),
            Some(_) => {
                return error(
                    format!("dielectric layer {} material must be a nonempty name", repr(&item.name)),
                    &item.location,
                );
            }
        };
        let construction = match item.parameters.get("type") {
            None => None,
            Some(Scalar::Str(text)) if text == "core" || text == "prepreg" => Some(text.clone()),
            Some(_) => {
                return error(
                    format!("dielectric layer {} type must be core or prepreg", repr(&item.name)),
                    &item.location,
                );
            }
        };
        layers.push(StackupLayer {
            id: item.name.clone(),
            copper_layer: None,
            thickness_nm: thickness,
            material,
            relative_permittivity: Some(er),
            loss_tangent: loss,
            dielectric_type: construction,
        });
    }
    if layers.last().is_some_and(|layer| layer.copper_layer.is_none()) {
        let last = declaration.layers.last().expect("nonempty");
        return error("stackup must end with a copper layer".to_owned(), &last.location);
    }
    let copper: Vec<CopperLayer> = layers.iter().filter_map(|layer| layer.copper_layer).collect();
    let internal = &CopperLayer::ALL[1..CopperLayer::ALL.len() - 1];
    let mut expected = vec![CopperLayer::Front];
    expected.extend(&internal[..copper.len().saturating_sub(2).min(internal.len())]);
    expected.push(CopperLayer::Back);
    if copper.len() < 2 || copper != expected {
        return error(
            "stackup copper layers must be F.Cu, In1.Cu, In2.Cu ... B.Cu in order from top to bottom".to_owned(),
            &declaration.location,
        );
    }
    Ok(MechanicalStackup { layers, location: declaration.location.clone() })
}

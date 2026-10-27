//! Python object semantics of parsed property values: `repr`, `str`,
//! truthiness and `==`, as the lowering error messages and checks see them.

use num_bigint::BigInt;
use num_traits::{FromPrimitive, Zero};

use copper_json::{python_float_repr, python_str_repr};

use crate::syntax::{Scalar, Value};

pub fn scalar_repr(value: &Scalar) -> String {
    match value {
        Scalar::Str(text) => python_str_repr(text),
        Scalar::Bool(true) => "True".to_owned(),
        Scalar::Bool(false) => "False".to_owned(),
        Scalar::Int(number) => number.to_string(),
        Scalar::Float(number) => python_float_repr(*number),
        Scalar::Quantity { value, unit } => {
            format!("RawQuantity(value={}, unit={})", python_str_repr(value), python_str_repr(unit))
        }
    }
}

fn pair_repr(x: &Scalar, y: &Scalar) -> String {
    format!("({}, {})", scalar_repr(x), scalar_repr(y))
}

/// `repr(value)`.
pub fn repr(value: &Value) -> String {
    match value {
        Value::Scalar(scalar) => scalar_repr(scalar),
        Value::Point(x, y) => pair_repr(x, y),
        Value::Points(points) if points.len() == 1 => format!("({},)", pair_repr(&points[0].0, &points[0].1)),
        Value::Points(points) => {
            format!("({})", points.iter().map(|(x, y)| pair_repr(x, y)).collect::<Vec<_>>().join(", "))
        }
        Value::List(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Map(_) | Value::PadRef(_) => format!("{value:?}"),
    }
}

/// `str(value)`: the text itself for strings, `repr` otherwise.
pub fn text(value: &Value) -> String {
    match value {
        Value::Scalar(Scalar::Str(text)) => text.clone(),
        other => repr(other),
    }
}

/// `bool(value)`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Scalar(Scalar::Str(text)) => !text.is_empty(),
        Value::Scalar(Scalar::Bool(flag)) => *flag,
        Value::Scalar(Scalar::Int(number)) => !number.is_zero(),
        Value::Scalar(Scalar::Float(number)) => *number != 0.0,
        Value::Scalar(Scalar::Quantity { .. }) | Value::Point(..) | Value::PadRef(_) => true,
        Value::Points(items) => !items.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(items) => !items.is_empty(),
    }
}

pub fn as_str(value: &Value) -> Option<&str> {
    match value {
        Value::Scalar(Scalar::Str(text)) => Some(text),
        _ => None,
    }
}

/// The exact numeric value of a bool, int or float, as a big integer when integral.
enum Number {
    Integer(BigInt),
    Fractional(f64),
}

fn number(value: &Scalar) -> Option<Number> {
    match value {
        Scalar::Bool(flag) => Some(Number::Integer(BigInt::from(u8::from(*flag)))),
        Scalar::Int(number) => Some(Number::Integer(number.clone())),
        Scalar::Float(number) if number.fract() == 0.0 => BigInt::from_f64(*number).map(Number::Integer),
        Scalar::Float(number) => Some(Number::Fractional(*number)),
        _ => None,
    }
}

pub fn scalar_eq(first: &Scalar, second: &Scalar) -> bool {
    match (first, second) {
        (Scalar::Str(a), Scalar::Str(b)) => a == b,
        (Scalar::Quantity { value: a, unit: u }, Scalar::Quantity { value: b, unit: v }) => a == b && u == v,
        _ => match (number(first), number(second)) {
            (Some(Number::Integer(a)), Some(Number::Integer(b))) => a == b,
            (Some(Number::Fractional(a)), Some(Number::Fractional(b))) => a == b,
            _ => false,
        },
    }
}

/// `first == second`, including `1 == 1.0 == True`.
pub fn eq(first: &Value, second: &Value) -> bool {
    match (first, second) {
        (Value::Scalar(a), Value::Scalar(b)) => scalar_eq(a, b),
        (Value::Point(a, b), Value::Point(c, d)) => scalar_eq(a, c) && scalar_eq(b, d),
        (Value::Points(a), Value::Points(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|((w, x), (y, z))| scalar_eq(w, y) && scalar_eq(x, z))
        }
        _ => first == second,
    }
}

/// `len(set(values))` under Python equality.
pub fn distinct_count<'a>(values: impl IntoIterator<Item = &'a Value>) -> usize {
    let mut seen: Vec<&Value> = Vec::new();
    for value in values {
        if !seen.iter().any(|other| eq(other, value)) {
            seen.push(value);
        }
    }
    seen.len()
}

/// `type(value).__name__`.
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Scalar(Scalar::Str(_)) => "str",
        Value::Scalar(Scalar::Bool(_)) => "bool",
        Value::Scalar(Scalar::Int(_)) => "int",
        Value::Scalar(Scalar::Float(_)) => "float",
        Value::Scalar(Scalar::Quantity { .. }) => "RawQuantity",
        Value::Point(..) | Value::Points(_) => "tuple",
        Value::List(_) => "list",
        Value::Map(_) => "dict",
        Value::PadRef(_) => "MacroPadReference",
    }
}

/// `Decimal(str(number))` for an int or float.
pub fn decimal(value: &Scalar) -> Option<copper_units::Decimal> {
    let text = match value {
        Scalar::Int(number) => number.to_string(),
        Scalar::Float(number) => python_float_repr(*number),
        _ => return None,
    };
    copper_units::Decimal::parse(&text).ok()
}

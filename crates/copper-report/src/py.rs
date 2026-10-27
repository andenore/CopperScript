//! Python's semantics for parsed JSON values: `str`, `repr`, truthiness,
//! `==`, `<`, iteration, `len`, slicing, `.get`, `[]`, `set` membership and
//! `str.join`, with the exceptions Python raises for the wrong types.
//!
//! Reports are loosely typed JSON read by Python code that assumes shapes
//! without checking them; the CLI turns some of the resulting exceptions into
//! an error line. Reproducing the exception class and message keeps those
//! lines identical.

use std::cmp::Ordering;
use std::fmt;

use copper_json::{Object, Value, python_float_repr, python_str_repr};
use num_bigint::BigInt;
use num_traits::{FromPrimitive, Zero};

/// A Python exception: its class name and `str(exception)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErr {
    pub kind: &'static str,
    pub message: String,
}

impl PyErr {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        PyErr { kind, message: message.into() }
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        PyErr::new("TypeError", message)
    }

    pub fn value_error(message: impl Into<String>) -> Self {
        PyErr::new("ValueError", message)
    }

    /// `KeyError(key)`: its `str` is the key's `repr`.
    pub fn key_error(key: &str) -> Self {
        PyErr::new("KeyError", python_str_repr(key))
    }

    /// Whether `except (classes)` catches this exception; subclasses
    /// (`FileNotFoundError` is an `OSError`, `UnicodeDecodeError` a
    /// `ValueError`) are caught by their base.
    pub fn is(&self, class: &str) -> bool {
        self.kind == class
            || match class {
                "OSError" => matches!(
                    self.kind,
                    "FileNotFoundError" | "IsADirectoryError" | "NotADirectoryError" | "PermissionError"
                ),
                "ValueError" => matches!(self.kind, "UnicodeDecodeError" | "JSONDecodeError"),
                "LookupError" => matches!(self.kind, "KeyError" | "IndexError"),
                _ => false,
            }
    }

    /// The last line of Python's traceback for this exception.
    pub fn crash_line(&self) -> String {
        if self.message.is_empty() { self.kind.to_owned() } else { format!("{}: {}", self.kind, self.message) }
    }
}

impl fmt::Display for PyErr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PyErr {}

pub type Py<T> = Result<T, PyErr>;

pub static NONE: Value = Value::Null;

pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `bool(value)`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Int(number) => !number.is_zero(),
        Value::Float(number) => *number != 0.0,
        Value::Str(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

/// `repr(value)`.
pub fn repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => python_float_repr(*number),
        Value::Str(text) => python_str_repr(text),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!("{}: {}", python_str_repr(key), repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str(value)`, which is also `f"{value}"` for these types.
pub fn text(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        other => repr(other),
    }
}

/// `str(value)[:limit]`, counting characters.
pub fn text_prefix(value: &Value, limit: usize) -> String {
    text(value).chars().take(limit).collect()
}

fn as_number(value: &Value) -> Option<Result<BigInt, f64>> {
    match value {
        Value::Bool(flag) => Some(Ok(BigInt::from(u8::from(*flag)))),
        Value::Int(number) => Some(Ok(number.clone())),
        Value::Float(number) => Some(Err(*number)),
        _ => None,
    }
}

/// Exact comparison of an int with a float, as Python compares them.
fn compare_int_float(int: &BigInt, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float > 0.0 { Ordering::Less } else { Ordering::Greater });
    }
    let floor = BigInt::from_f64(float.floor()).expect("finite");
    if float.fract() == 0.0 {
        return Some(int.cmp(&floor));
    }
    // A non-integral float lies strictly between floor and floor + 1.
    Some(if *int <= floor { Ordering::Less } else { Ordering::Greater })
}

fn compare_numbers(left: &Result<BigInt, f64>, right: &Result<BigInt, f64>) -> Option<Ordering> {
    match (left, right) {
        (Ok(a), Ok(b)) => Some(a.cmp(b)),
        (Err(a), Err(b)) => a.partial_cmp(b),
        (Ok(a), Err(b)) => compare_int_float(a, *b),
        (Err(a), Ok(b)) => compare_int_float(b, *a).map(Ordering::reverse),
    }
}

/// `left == right`.
pub fn eq(left: &Value, right: &Value) -> bool {
    if let (Some(a), Some(b)) = (as_number(left), as_number(right)) {
        return compare_numbers(&a, &b) == Some(Ordering::Equal);
    }
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| eq(x, y)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(key, value)| b.get(key).is_some_and(|other| eq(value, other)))
        }
        _ => false,
    }
}

/// `left == text`.
pub fn eq_str(value: &Value, text: &str) -> bool {
    value.as_str() == Some(text)
}

fn unsupported(op: &str, left: &Value, right: &Value) -> PyErr {
    PyErr::type_error(format!(
        "'{op}' not supported between instances of '{}' and '{}'",
        type_name(left),
        type_name(right)
    ))
}

/// `left < right`; `op` names the operator in the error (`<` or `>`, with
/// the operands in source order).
pub fn less(left: &Value, right: &Value, op: &str) -> Py<bool> {
    if let (Some(a), Some(b)) = (as_number(left), as_number(right)) {
        return Ok(compare_numbers(&a, &b) == Some(Ordering::Less));
    }
    match (left, right) {
        (Value::Str(a), Value::Str(b)) => Ok(a < b),
        (Value::Array(a), Value::Array(b)) => {
            // Sequences compare at the first unequal item.
            match a.iter().zip(b).find(|(x, y)| !eq(x, y)) {
                Some((x, y)) => less(x, y, "<"),
                None => Ok(a.len() < b.len()),
            }
        }
        _ => Err(unsupported(op, left, right)),
    }
}

/// `left > right`, raising as Python words it.
pub fn greater(left: &Value, right: &Value) -> Py<bool> {
    match less(right, left, "<") {
        Ok(result) => Ok(result),
        Err(_) => Err(unsupported(">", left, right)),
    }
}

/// `hash(value)` succeeds.
pub fn hashable(value: &Value) -> Py<()> {
    match value {
        Value::Array(_) | Value::Object(_) => {
            Err(PyErr::type_error(format!("unhashable type: '{}'", type_name(value))))
        }
        _ => Ok(()),
    }
}

/// `value in {"a", "b"}`: hashes first, so a list or dict raises.
pub fn in_str_set(value: &Value, set: &[&str]) -> Py<bool> {
    hashable(value)?;
    Ok(value.as_str().is_some_and(|text| set.contains(&text)))
}

fn object_of<'a>(value: &'a Value, attribute: &str) -> Py<&'a Object> {
    value.as_object().ok_or_else(|| {
        PyErr::new("AttributeError", format!("'{}' object has no attribute '{attribute}'", type_name(value)))
    })
}

/// `value.get(key)`: `None` for a missing key.
pub fn get<'a>(value: &'a Value, key: &str) -> Py<&'a Value> {
    Ok(object_of(value, "get")?.get(key).unwrap_or(&NONE))
}

/// `value.get(key, default)`.
pub fn get_or<'a>(value: &'a Value, key: &str, default: &'a Value) -> Py<&'a Value> {
    Ok(object_of(value, "get")?.get(key).unwrap_or(default))
}

/// `value[key]` where the shape is known (documents this crate built):
/// `None` when absent.
pub fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NONE)
}

/// `value[key]` for a string key.
pub fn item<'a>(value: &'a Value, key: &str) -> Py<&'a Value> {
    match value {
        Value::Object(object) => object.get(key).ok_or_else(|| PyErr::key_error(key)),
        Value::Array(_) => Err(PyErr::type_error("list indices must be integers or slices, not str")),
        Value::Str(_) => Err(PyErr::type_error("string indices must be integers, not 'str'")),
        other => Err(PyErr::type_error(format!("'{}' object is not subscriptable", type_name(other)))),
    }
}

/// `value[index]` for a non-negative integer index.
pub fn index(value: &Value, index: usize) -> Py<Value> {
    match value {
        Value::Array(items) => {
            items.get(index).cloned().ok_or_else(|| PyErr::new("IndexError", "list index out of range"))
        }
        Value::Str(text) => text
            .chars()
            .nth(index)
            .map(|character| Value::from(character.to_string()))
            .ok_or_else(|| PyErr::new("IndexError", "string index out of range")),
        // JSON object keys are strings, so an integer key is never present.
        Value::Object(_) => Err(PyErr::new("KeyError", index.to_string())),
        other => Err(PyErr::type_error(format!("'{}' object is not subscriptable", type_name(other)))),
    }
}

/// `iter(value)`, collected: list items, dict keys, string characters.
pub fn iterate(value: &Value) -> Py<Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(object) => Ok(object.keys().map(|key| Value::from(key.as_str())).collect()),
        Value::Str(text) => Ok(text.chars().map(|character| Value::from(character.to_string())).collect()),
        other => Err(PyErr::type_error(format!("'{}' object is not iterable", type_name(other)))),
    }
}

/// `len(value)`.
pub fn len(value: &Value) -> Py<usize> {
    match value {
        Value::Array(items) => Ok(items.len()),
        Value::Object(object) => Ok(object.len()),
        Value::Str(text) => Ok(text.chars().count()),
        other => Err(PyErr::type_error(format!("object of type '{}' has no len()", type_name(other)))),
    }
}

/// `value[:stop]`.
pub fn head(value: &Value, stop: usize) -> Py<Value> {
    match value {
        Value::Array(items) => Ok(Value::Array(items.iter().take(stop).cloned().collect())),
        Value::Str(text) => Ok(Value::from(text.chars().take(stop).collect::<String>())),
        Value::Object(_) => Err(PyErr::new("KeyError", format!("slice(None, {stop}, None)"))),
        other => Err(PyErr::type_error(format!("'{}' object is not subscriptable", type_name(other)))),
    }
}

/// `list(value)`.
pub fn list(value: &Value) -> Py<Vec<Value>> {
    iterate(value)
}

/// `separator.join(value)`.
pub fn join(separator: &str, value: &Value) -> Py<String> {
    let items = match value {
        Value::Array(_) | Value::Object(_) | Value::Str(_) => iterate(value)?,
        _ => return Err(PyErr::type_error("can only join an iterable")),
    };
    let mut parts = Vec::with_capacity(items.len());
    for (position, item) in items.iter().enumerate() {
        match item {
            Value::Str(text) => parts.push(text.clone()),
            other => {
                return Err(PyErr::type_error(format!(
                    "sequence item {position}: expected str instance, {} found",
                    type_name(other)
                )));
            }
        }
    }
    Ok(parts.join(separator))
}

/// `set(value)` as a list of distinct items in first-seen order.
pub fn set(value: &Value) -> Py<Vec<Value>> {
    let mut items: Vec<Value> = Vec::new();
    for item in iterate(value)? {
        hashable(&item)?;
        if !items.iter().any(|seen| eq(seen, &item)) {
            items.push(item);
        }
    }
    Ok(items)
}

/// `item in container` (`not in` negates it).
pub fn contains(container: &Value, item: &Value) -> Py<bool> {
    match container {
        Value::Array(items) => Ok(items.iter().any(|candidate| eq(candidate, item))),
        Value::Object(object) => {
            hashable(item)?;
            Ok(item.as_str().is_some_and(|key| object.contains_key(key)))
        }
        Value::Str(text) => match item {
            Value::Str(part) => Ok(text.contains(part.as_str())),
            other => Err(PyErr::type_error(format!(
                "'in <string>' requires string as left operand, not {}",
                type_name(other)
            ))),
        },
        other => Err(PyErr::type_error(format!("argument of type '{}' is not iterable", type_name(other)))),
    }
}

/// `start + value` for numbers, as `sum()` adds them.
pub fn add(left: &Value, right: &Value) -> Py<Value> {
    match (as_number(left), as_number(right)) {
        (Some(Ok(a)), Some(Ok(b))) => Ok(Value::Int(a + b)),
        (Some(a), Some(b)) => {
            let float = |number: Result<BigInt, f64>| match number {
                Ok(int) => num_traits::ToPrimitive::to_f64(&int).unwrap_or(f64::INFINITY),
                Err(float) => float,
            };
            Ok(Value::Float(float(a) + float(b)))
        }
        _ => Err(PyErr::type_error(format!(
            "unsupported operand type(s) for +: '{}' and '{}'",
            type_name(left),
            type_name(right)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Value {
        copper_json::parse(text).unwrap()
    }

    #[test]
    fn text_and_repr_follow_python() {
        assert_eq!(text(&json(r#"{"a": [1, 2.5, true, null, "it's"]}"#)), r#"{'a': [1, 2.5, True, None, "it's"]}"#);
        assert_eq!(text(&json("1e16")), "1e+16");
    }

    #[test]
    fn comparisons_and_errors() {
        assert!(eq(&json("1"), &json("1.0")));
        assert!(eq(&json("true"), &json("1")));
        assert!(less(&json("[1, 2]"), &json("[1, 3]"), "<").unwrap());
        assert_eq!(
            less(&json("1"), &json("\"a\""), "<").unwrap_err().message,
            "'<' not supported between instances of 'int' and 'str'"
        );
        assert_eq!(
            greater(&json("\"a\""), &json("1")).unwrap_err().message,
            "'>' not supported between instances of 'str' and 'int'"
        );
        assert_eq!(
            join(",", &json("[\"a\", 1]")).unwrap_err().message,
            "sequence item 1: expected str instance, int found"
        );
        assert_eq!(get(&json("[]"), "x").unwrap_err().message, "'list' object has no attribute 'get'");
        assert_eq!(head(&json("{}"), 3).unwrap_err().message, "slice(None, 3, None)");
    }
}

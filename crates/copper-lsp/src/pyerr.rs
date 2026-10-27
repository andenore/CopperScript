//! The Python exceptions a request handler can raise, and the subscript and
//! truthiness rules of the JSON values it reads.
//!
//! `pcbir.lsp.server` answers a failed request with a JSON-RPC error whose
//! code depends on the exception class and whose message is `str(exc)`, so
//! malformed parameters must fail with Python's own message.

use copper_json::{Value, python_str_repr};

/// A Python exception: its class decides the JSON-RPC error code.
#[derive(Clone, Debug, PartialEq)]
pub enum PyError {
    /// `KeyError(key)`; `str()` is the key's repr.
    Key(String),
    Type(String),
    Value(String),
    /// `OSError`, already formatted like `str(exc)`.
    Os(String),
    /// A `LookupError` other than `KeyError` (`IndexError`, or the server's own).
    Lookup(String),
    NotImplemented(String),
    Attribute(String),
}

impl PyError {
    pub fn key(key: &str) -> PyError {
        PyError::Key(python_str_repr(key))
    }

    /// The JSON-RPC error code of `Server.dispatch`.
    pub fn code(&self) -> i64 {
        match self {
            PyError::NotImplemented(_) => -32601,
            PyError::Lookup(_) => -32002,
            PyError::Key(_) | PyError::Type(_) | PyError::Value(_) | PyError::Os(_) => -32602,
            PyError::Attribute(_) => -32603,
        }
    }

    /// `str(exc)`.
    pub fn message(&self) -> &str {
        match self {
            PyError::Key(text)
            | PyError::Type(text)
            | PyError::Value(text)
            | PyError::Os(text)
            | PyError::Lookup(text)
            | PyError::NotImplemented(text)
            | PyError::Attribute(text) => text,
        }
    }
}

pub type PyResult<T> = Result<T, PyError>;

/// `type(value).__name__`.
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

/// `value[key]` for a string key.
pub fn item<'a>(value: &'a Value, key: &str) -> PyResult<&'a Value> {
    match value {
        Value::Object(object) => object.get(key).ok_or_else(|| PyError::key(key)),
        Value::Str(_) => Err(PyError::Type("string indices must be integers, not 'str'".to_owned())),
        Value::Array(_) => Err(PyError::Type("list indices must be integers or slices, not str".to_owned())),
        other => Err(PyError::Type(format!("'{}' object is not subscriptable", type_name(other)))),
    }
}

/// `value.get(key)`: `None` (here `Ok(None)`) for a missing key.
pub fn get<'a>(value: &'a Value, key: &str) -> PyResult<Option<&'a Value>> {
    match value {
        Value::Object(object) => Ok(object.get(key)),
        other => Err(PyError::Attribute(format!("'{}' object has no attribute 'get'", type_name(other)))),
    }
}

/// `bool(value)`.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Int(number) => number.sign() != num_bigint::Sign::NoSign,
        Value::Float(number) => *number != 0.0,
        Value::Str(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

/// The items `for item in value` visits: list items, dict keys or the
/// characters of a string.
pub fn iterate(value: &Value) -> PyResult<Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(object) => Ok(object.keys().map(|key| Value::Str(key.clone())).collect()),
        Value::Str(text) => Ok(text.chars().map(|c| Value::Str(c.to_string())).collect()),
        other => Err(PyError::Type(format!("'{}' object is not iterable", type_name(other)))),
    }
}

/// `type(value) is int`: a JSON integer (Python's `bool` is not `int` here).
pub fn exact_int(value: &Value) -> Option<&num_bigint::BigInt> {
    match value {
        Value::Int(number) => Some(number),
        _ => None,
    }
}

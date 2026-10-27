//! JSON values, parsing and serialization compatible with Python's `json` module.
//!
//! CopperScript's reports, state files and CLI output are written by Python's
//! `json.dumps`. This crate reproduces those bytes: key order is kept as parsed
//! (like a Python `dict`), floats print like Python's `repr`, and strings escape
//! like `ensure_ascii`. See [`Dumps`] for the serializer options.

mod float;
pub mod oserror;
mod parse;
mod serialize;
pub mod text;

pub use float::python_float_repr;
pub use parse::{ParseError, Rejection, StrictError, UniqueError, parse, parse_strict, parse_unique};
pub use serialize::{Dumps, encode_str};

use indexmap::IndexMap;
use num_bigint::BigInt;

/// A JSON object: insertion-ordered, a repeated key keeps its first position and
/// its last value, as in a Python `dict`.
pub type Object = IndexMap<String, Value>;

/// A JSON value as Python's `json.loads` produces it.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    /// An integer of any size; Python ints are unbounded.
    Int(BigInt),
    /// A float, including the NaN and infinities Python reads and writes.
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Object),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The value at `key` when this is an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|object| object.get(key))
    }
}

/// Structural equality; floats compare by bit pattern so NaN equals itself.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|((ka, va), (kb, vb))| ka == kb && va == vb)
            }
            _ => false,
        }
    }
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Value::Str(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Value::Str(text)
    }
}

impl From<bool> for Value {
    fn from(flag: bool) -> Self {
        Value::Bool(flag)
    }
}

impl From<i64> for Value {
    fn from(number: i64) -> Self {
        Value::Int(BigInt::from(number))
    }
}

impl From<u64> for Value {
    fn from(number: u64) -> Self {
        Value::Int(BigInt::from(number))
    }
}

impl From<BigInt> for Value {
    fn from(number: BigInt) -> Self {
        Value::Int(number)
    }
}

impl From<f64> for Value {
    fn from(number: f64) -> Self {
        Value::Float(number)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::Array(items)
    }
}

impl From<Object> for Value {
    fn from(object: Object) -> Self {
        Value::Object(object)
    }
}

mod unicode;
pub use unicode::{
    python_casefold, python_isalnum, python_isalpha, python_isdigit, python_isprintable, python_isspace,
    python_str_repr,
};

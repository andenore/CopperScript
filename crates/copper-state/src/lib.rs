//! The `copperscript.state/v1` file format, frozen as the cross-language contract.
//!
//! A state file is what `pcbir.state_codec.save_state` writes: one line of
//! compact JSON, `"schema"` first, then `content_digest`, `kind`, `metadata` and
//! `value` with sorted keys, and a final newline. The digest is the SHA-256 of
//! the compact, key-sorted, ASCII-escaped JSON of `value`. Values are tagged:
//!
//! | Tag | Python value |
//! |---|---|
//! | `{"$type": "pcbir.mod:Qual", "fields": {...}}` | a frozen dataclass defined in a `pcbir` module |
//! | `{"$enum": "pcbir.mod:Qual", "value": v}` | an enum member defined in a `pcbir` module |
//! | `{"$decimal": "1.50"}` | `Decimal`, as `str()` |
//! | `{"$fraction": [n, d]}` | `Fraction` in lowest terms |
//! | `{"$tuple": [...]}` | tuple |
//! | `{"$frozenset": [...]}`, `{"$set": [...]}` | sets, items sorted by their key-sorted JSON |
//! | `{"$map": [[k, v], ...], "proxy": bool}` | dict or `MappingProxyType` |
//! | `{"$ndarray": {"dtype": "<f8", "shape": [...], "data": base64}}` | numeric numpy array, little-endian |
//! | plain JSON | `None`, bool, int, float, str, list |
//!
//! Changing any of this needs a new schema version. Python's reader also
//! accepts an untagged object as a plain dict; [`StateValue::Object`] keeps
//! that, although the writer never produces it.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use copper_json::{Dumps, Object, Value};
use copper_units::{Decimal, Fraction, fraction};
use indexmap::IndexMap;
use num_bigint::BigInt;
use sha2::{Digest, Sha256};

pub const STATE_SCHEMA: &str = "copperscript.state/v1";
/// Only types defined in this Python package (or its submodules) may appear.
const ALLOWED_PREFIX: &str = "pcbir";
const ARRAY_KINDS: &str = "biuf";

/// Why a document is not a valid state file.
#[derive(Clone, Debug, PartialEq)]
pub enum StateError {
    Json(copper_json::ParseError),
    /// Python: `"{path}: not a copperscript.state/v1 file"`.
    NotAStateFile,
    /// Python: `"{path}: content digest mismatch"`.
    DigestMismatch,
    /// A field Python's reader requires is missing or has the wrong type.
    Malformed(String),
    /// A `$type`/`$enum` name outside the `pcbir` package.
    ForeignType(String),
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Json(error) => error.fmt(formatter),
            StateError::NotAStateFile => write!(formatter, "not a {STATE_SCHEMA} file"),
            StateError::DigestMismatch => formatter.write_str("content digest mismatch"),
            StateError::Malformed(message) => formatter.write_str(message),
            StateError::ForeignType(name) => {
                write!(formatter, "refusing to construct {name:?}: only pcbir types are allowed")
            }
        }
    }
}

impl std::error::Error for StateError {}

fn malformed(message: impl Into<String>) -> StateError {
    StateError::Malformed(message.into())
}

/// A numeric numpy array: little-endian bytes in C order.
#[derive(Clone, Debug, PartialEq)]
pub struct NdArray {
    pub dtype: String,
    pub shape: Vec<u64>,
    pub data: Vec<u8>,
}

impl NdArray {
    fn validate(&self) -> Result<(), StateError> {
        let dtype = self.dtype.trim_start_matches(['<', '>', '|', '=']);
        let mut chars = dtype.chars();
        let kind = chars.next().ok_or_else(|| malformed(format!("unsupported array dtype {:?}", self.dtype)))?;
        let size: u64 =
            chars.as_str().parse().map_err(|_| malformed(format!("unsupported array dtype {:?}", self.dtype)))?;
        if !ARRAY_KINDS.contains(kind) || size == 0 || self.dtype.len() - dtype.len() > 1 {
            return Err(malformed(format!("unsupported array dtype {:?}", self.dtype)));
        }
        let count = self.shape.iter().try_fold(1u64, |total, size| total.checked_mul(*size));
        if count.and_then(|count| count.checked_mul(size)) != Some(self.data.len() as u64) {
            return Err(malformed("array data does not match its shape and dtype"));
        }
        Ok(())
    }
}

/// A decoded state value; see the module documentation for the tags.
#[derive(Clone, Debug, PartialEq)]
pub enum StateValue {
    Null,
    Bool(bool),
    Int(BigInt),
    Float(f64),
    Str(String),
    List(Vec<StateValue>),
    Dataclass {
        name: String,
        fields: IndexMap<String, StateValue>,
    },
    Enum {
        name: String,
        value: Box<StateValue>,
    },
    Decimal(Decimal),
    Fraction(Fraction),
    Tuple(Vec<StateValue>),
    FrozenSet(Vec<StateValue>),
    Set(Vec<StateValue>),
    Map {
        entries: Vec<(StateValue, StateValue)>,
        proxy: bool,
    },
    NdArray(NdArray),
    /// An untagged object; read for compatibility, never written by Python.
    Object(IndexMap<String, StateValue>),
}

fn check_type_name(name: &str) -> Result<(), StateError> {
    let (module, qualname) = name.split_once(':').unwrap_or((name, ""));
    let in_package = module == ALLOWED_PREFIX || module.starts_with(&format!("{ALLOWED_PREFIX}."));
    if !in_package || qualname.is_empty() { Err(StateError::ForeignType(name.to_owned())) } else { Ok(()) }
}

fn tag_text<'a>(object: &'a Object, key: &str) -> Result<&'a str, StateError> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| malformed(format!("{key} must be a string")))
}

fn tag_items<'a>(object: &'a Object, key: &str) -> Result<&'a [Value], StateError> {
    object.get(key).and_then(Value::as_array).ok_or_else(|| malformed(format!("{key} must be a list")))
}

impl StateValue {
    /// Python's `state_codec.decode`, checking tags in the same order.
    pub fn decode(value: &Value) -> Result<StateValue, StateError> {
        let object = match value {
            Value::Null => return Ok(StateValue::Null),
            Value::Bool(flag) => return Ok(StateValue::Bool(*flag)),
            Value::Int(number) => return Ok(StateValue::Int(number.clone())),
            Value::Float(number) => return Ok(StateValue::Float(*number)),
            Value::Str(text) => return Ok(StateValue::Str(text.clone())),
            Value::Array(items) => return Ok(StateValue::List(decode_all(items)?)),
            Value::Object(object) => object,
        };
        if object.contains_key("$type") {
            let name = tag_text(object, "$type")?;
            check_type_name(name)?;
            let fields = object
                .get("fields")
                .and_then(Value::as_object)
                .ok_or_else(|| malformed(format!("{name} needs a fields object")))?;
            let fields = fields
                .iter()
                .map(|(key, item)| Ok((key.clone(), StateValue::decode(item)?)))
                .collect::<Result<_, StateError>>()?;
            return Ok(StateValue::Dataclass { name: name.to_owned(), fields });
        }
        if object.contains_key("$enum") {
            let name = tag_text(object, "$enum")?;
            check_type_name(name)?;
            let item = object.get("value").ok_or_else(|| malformed(format!("{name} needs a value")))?;
            return Ok(StateValue::Enum { name: name.to_owned(), value: Box::new(StateValue::decode(item)?) });
        }
        if object.contains_key("$decimal") {
            let text = tag_text(object, "$decimal")?;
            return Decimal::parse(text).map(StateValue::Decimal).map_err(|error| malformed(error.to_string()));
        }
        if object.contains_key("$fraction") {
            let parts = tag_items(object, "$fraction")?;
            let integer = |item: &Value| match item {
                Value::Int(number) => Ok(number.clone()),
                _ => Err(malformed("$fraction must hold two integers")),
            };
            let [numerator, denominator] = parts else {
                return Err(malformed("$fraction must hold two integers"));
            };
            return fraction(integer(numerator)?, integer(denominator)?)
                .map(StateValue::Fraction)
                .ok_or_else(|| malformed("$fraction has a zero denominator"));
        }
        if object.contains_key("$tuple") {
            return Ok(StateValue::Tuple(decode_all(tag_items(object, "$tuple")?)?));
        }
        if object.contains_key("$frozenset") {
            return Ok(StateValue::FrozenSet(decode_all(tag_items(object, "$frozenset")?)?));
        }
        if object.contains_key("$set") {
            return Ok(StateValue::Set(decode_all(tag_items(object, "$set")?)?));
        }
        if let Some(spec) = object.get("$ndarray") {
            let spec = spec.as_object().ok_or_else(|| malformed("$ndarray must be an object"))?;
            let shape = tag_items(spec, "shape")?
                .iter()
                .map(|size| match size {
                    Value::Int(size) => {
                        u64::try_from(size.clone()).map_err(|_| malformed("array shape must be non-negative"))
                    }
                    _ => Err(malformed("array shape must hold integers")),
                })
                .collect::<Result<_, _>>()?;
            let data = BASE64
                .decode(tag_text(spec, "data")?)
                .map_err(|error| malformed(format!("array data is not base64: {error}")))?;
            let array = NdArray { dtype: tag_text(spec, "dtype")?.to_owned(), shape, data };
            array.validate()?;
            return Ok(StateValue::NdArray(array));
        }
        if object.contains_key("$map") {
            let entries = tag_items(object, "$map")?
                .iter()
                .map(|pair| match pair.as_array() {
                    Some([key, item]) => Ok((StateValue::decode(key)?, StateValue::decode(item)?)),
                    _ => Err(malformed("$map entries must be [key, value] pairs")),
                })
                .collect::<Result<_, _>>()?;
            let proxy = match object.get("proxy") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(flag)) => *flag,
                Some(_) => return Err(malformed("$map proxy must be a boolean")),
            };
            return Ok(StateValue::Map { entries, proxy });
        }
        let fields = object
            .iter()
            .map(|(key, item)| Ok((key.clone(), StateValue::decode(item)?)))
            .collect::<Result<_, StateError>>()?;
        Ok(StateValue::Object(fields))
    }

    /// Python's `state_codec.encode` of the equivalent value.
    pub fn encode(&self) -> Value {
        let tagged = |tag: &str, value: Value| Value::Object(Object::from([(tag.to_owned(), value)]));
        match self {
            StateValue::Null => Value::Null,
            StateValue::Bool(flag) => Value::Bool(*flag),
            StateValue::Int(number) => Value::Int(number.clone()),
            StateValue::Float(number) => Value::Float(*number),
            StateValue::Str(text) => Value::Str(text.clone()),
            StateValue::List(items) => Value::Array(items.iter().map(StateValue::encode).collect()),
            StateValue::Dataclass { name, fields } => Value::Object(Object::from([
                ("$type".to_owned(), Value::from(name.as_str())),
                (
                    "fields".to_owned(),
                    Value::Object(fields.iter().map(|(key, item)| (key.clone(), item.encode())).collect()),
                ),
            ])),
            StateValue::Enum { name, value } => Value::Object(Object::from([
                ("$enum".to_owned(), Value::from(name.as_str())),
                ("value".to_owned(), value.encode()),
            ])),
            StateValue::Decimal(number) => tagged("$decimal", Value::from(number.to_sci_string())),
            StateValue::Fraction(number) => tagged(
                "$fraction",
                Value::Array(vec![Value::Int(number.numer().clone()), Value::Int(number.denom().clone())]),
            ),
            StateValue::Tuple(items) => tagged("$tuple", Value::Array(items.iter().map(StateValue::encode).collect())),
            StateValue::FrozenSet(items) => tagged("$frozenset", sorted_set(items)),
            StateValue::Set(items) => tagged("$set", sorted_set(items)),
            StateValue::Map { entries, proxy } => Value::Object(Object::from([
                (
                    "$map".to_owned(),
                    Value::Array(
                        entries.iter().map(|(key, item)| Value::Array(vec![key.encode(), item.encode()])).collect(),
                    ),
                ),
                ("proxy".to_owned(), Value::Bool(*proxy)),
            ])),
            StateValue::NdArray(array) => tagged(
                "$ndarray",
                Value::Object(Object::from([
                    ("dtype".to_owned(), Value::from(array.dtype.as_str())),
                    ("shape".to_owned(), Value::Array(array.shape.iter().map(|size| Value::from(*size)).collect())),
                    ("data".to_owned(), Value::from(BASE64.encode(&array.data))),
                ])),
            ),
            StateValue::Object(fields) => {
                Value::Object(fields.iter().map(|(key, item)| (key.clone(), item.encode())).collect())
            }
        }
    }
}

fn decode_all(items: &[Value]) -> Result<Vec<StateValue>, StateError> {
    items.iter().map(StateValue::decode).collect()
}

/// Set items in Python's order: by `json.dumps(item, sort_keys=True)`.
fn sorted_set(items: &[StateValue]) -> Value {
    let dumps = Dumps::new().sort_keys(true);
    let mut keyed: Vec<(String, Value)> = items
        .iter()
        .map(|item| {
            let encoded = item.encode();
            (dumps.dumps(&encoded), encoded)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    Value::Array(keyed.into_iter().map(|(_, value)| value).collect())
}

/// `"sha256:" + sha256(compact, key-sorted JSON of value)`.
pub fn content_digest(value: &Value) -> String {
    let body = Dumps::compact().sort_keys(true).dumps(value);
    let digest = Sha256::digest(body.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// A verified, decoded state file.
#[derive(Clone, Debug, PartialEq)]
pub struct StateDocument {
    /// The Python `module:qualname` of the saved value's type.
    pub kind: String,
    /// JSON metadata saved with the value.
    pub metadata: Object,
    pub value: StateValue,
}

fn parse_document(text: &str) -> Result<Object, StateError> {
    match copper_json::parse(text).map_err(StateError::Json)? {
        Value::Object(document) if document.get("schema").and_then(Value::as_str) == Some(STATE_SCHEMA) => Ok(document),
        Value::Object(_) => Err(StateError::NotAStateFile),
        _ => Err(malformed("a state file is a JSON object")),
    }
}

impl StateDocument {
    pub fn new(kind: impl Into<String>, metadata: Object, value: StateValue) -> Self {
        StateDocument { kind: kind.into(), metadata, value }
    }

    /// Python's `load_state(path, with_metadata=True)`: schema, digest, decode.
    pub fn read(text: &str) -> Result<StateDocument, StateError> {
        let document = parse_document(text)?;
        let value = document.get("value").ok_or_else(|| malformed("a state file needs a value"))?;
        if document.get("content_digest").and_then(Value::as_str) != Some(content_digest(value).as_str()) {
            return Err(StateError::DigestMismatch);
        }
        let kind = document.get("kind").and_then(Value::as_str).ok_or_else(|| malformed("kind must be a string"))?;
        let metadata = match document.get("metadata") {
            None => Object::new(),
            Some(Value::Object(metadata)) => metadata.clone(),
            Some(_) => return Err(malformed("metadata must be an object")),
        };
        Ok(StateDocument { kind: kind.to_owned(), metadata, value: StateValue::decode(value)? })
    }

    /// The exact text Python's `save_state` writes, ending in `"\n"`. Write it
    /// with [`copper_json::text::write_text`] to get Python's newline on
    /// every platform.
    pub fn to_text(&self) -> String {
        let payload = self.value.encode();
        let rest = Object::from([
            ("content_digest".to_owned(), Value::from(content_digest(&payload))),
            ("kind".to_owned(), Value::from(self.kind.as_str())),
            ("metadata".to_owned(), Value::Object(self.metadata.clone())),
            ("value".to_owned(), payload),
        ]);
        let rest = Dumps::compact().sort_keys(true).dumps(&Value::Object(rest));
        format!("{{\"schema\":\"{STATE_SCHEMA}\",{}\n", &rest[1..])
    }
}

/// Python's `is_state_file`: the schema is the first key.
pub fn is_state_text(text: &str) -> bool {
    text.starts_with(&format!("{{\"schema\":\"{STATE_SCHEMA}\""))
}

/// Python's `describe_state` (no digest check): kind, metadata, digest and size.
pub fn describe(text: &str, size_bytes: u64) -> Result<Value, StateError> {
    let document = parse_document(text)?;
    let field = |key: &str| document.get(key).cloned().unwrap_or(Value::Null);
    Ok(Value::Object(Object::from([
        ("kind".to_owned(), field("kind")),
        ("metadata".to_owned(), document.get("metadata").cloned().unwrap_or(Value::Object(Object::new()))),
        ("content_digest".to_owned(), field("content_digest")),
        ("bytes".to_owned(), Value::from(size_bytes)),
    ])))
}

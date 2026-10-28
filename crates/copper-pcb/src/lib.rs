//! Physical board IR, electrical-to-physical lowering, hard macros and the
//! KiCad PCB backend (`pcbir.physical`, `pcbir.physicalize`,
//! `pcbir.hard_macros`, `pcbir.backends.kicad_pcb`).
//!
//! Errors carry the Python `ValueError` message. Physical behaviour this
//! rewrite does not yet cover (via-in-pad arrays, macro edge openings)
//! reports [`Error::NotImplemented`] instead of guessing. Hard-macro
//! materialization, which needs physical DRC, is in `copper-drc`.

macro_rules! text_enum {
    ($name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<$name> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }
        }
    };
}

pub mod board;
pub mod hard_macros;
pub mod keepouts;
pub mod kicad_pcb;
pub mod physicalize;
pub mod power;
pub mod project;
pub mod state_codec;
pub mod templates;
pub mod transform;
pub mod zones;

use copper_units::{Decimal, Rounding};

/// A `ValueError` (by its message), or behaviour not yet rewritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Value(String),
    NotImplemented(String),
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error::Value(message)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Value(message) | Error::NotImplemented(message) => formatter.write_str(message),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

pub fn not_implemented<T>(what: &str) -> Result<T> {
    Err(Error::NotImplemented(what.to_owned()))
}

pub(crate) fn decimal_error(error: copper_units::DecimalError) -> Error {
    Error::Value(error.to_string())
}

/// Python's `Decimal(value) % modulus`: the remainder with the dividend's sign.
pub fn decimal_rem(value: &Decimal, modulus: i64) -> Result<Decimal> {
    value.rem(&Decimal::from_i64(modulus)).map_err(decimal_error)
}

pub(crate) fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items.iter().enumerate().any(|(index, item)| items[..index].contains(item))
}

/// Python's `str.strip()`.
pub(crate) fn py_strip(text: &str) -> &str {
    text.trim_matches(copper_json::python_isspace)
}

/// `nm_from_mm(text)`: exact millimetres to nanometres, rounding half up.
pub fn nm_from_mm_text(text: &str) -> Result<i64> {
    let value = Decimal::parse(text).map_err(decimal_error)?;
    let nm = value.mul(&Decimal::from_i64(1_000_000)).map_err(decimal_error)?.to_integral_value(Rounding::HalfUp);
    nm.to_i64().ok_or_else(|| Error::NotImplemented(format!("length {text} mm")))
}

/// `nm_from_mm(float)`: through the float's `repr`, as Python's `Decimal(str(x))`.
pub fn nm_from_mm_float(value: f64) -> Result<i64> {
    nm_from_mm_text(&copper_json::python_float_repr(value))
}

/// `ZoneConnection(text)`.
pub(crate) fn zone_connection(text: &str) -> Option<copper_physical::footprint::ZoneConnection> {
    use copper_physical::footprint::ZoneConnection;
    [ZoneConnection::Thermal, ZoneConnection::Solid, ZoneConnection::None, ZoneConnection::ThtThermal]
        .into_iter()
        .find(|connection| connection.value() == text)
}

/// `Decimal(str(x))` of a Python float.
pub fn float_decimal(value: f64) -> Result<Decimal> {
    Decimal::parse(&copper_json::python_float_repr(value)).map_err(decimal_error)
}

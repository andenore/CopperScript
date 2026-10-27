//! Board artwork intent (`pcbir.silkscreen`).
//!
//! Only the default design is rewritten so far; a `silkscreen` block reports
//! the rewrite placeholder error (see [`crate::mechanical::NOT_IMPLEMENTED`]).

use copper_units::Decimal;

use crate::error::CopperScriptError;
use crate::mechanical::NOT_IMPLEMENTED;
use crate::syntax::{Declaration, Document};

/// `SilkscreenBranding`: the Cu mark and compiler version label.
#[derive(Clone, Debug, PartialEq)]
pub struct SilkscreenBranding {
    pub enabled: bool,
    pub position: Option<(i64, i64)>,
    pub size_nm: i64,
    pub rotation_degrees: Decimal,
    pub side: &'static str,
    pub version: String,
    pub material: &'static str,
    pub exposed: bool,
    pub layout: String,
}

impl Default for SilkscreenBranding {
    fn default() -> Self {
        SilkscreenBranding {
            enabled: true,
            position: None,
            size_nm: 5_000_000,
            rotation_degrees: Decimal::zero(),
            side: "front",
            version: compiler_version(),
            material: "silkscreen",
            exposed: false,
            layout: "horizontal".to_owned(),
        }
    }
}

/// `compiler_version()`: the installed package version.
pub fn compiler_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SilkscreenDesign {
    pub branding: SilkscreenBranding,
}

pub fn lower_silkscreen(document: &Document) -> Result<SilkscreenDesign, CopperScriptError> {
    match document.declarations.iter().find(|declaration| matches!(declaration, Declaration::Silkscreen(_))) {
        None => Ok(SilkscreenDesign::default()),
        Some(Declaration::Silkscreen(block)) => {
            Err(CopperScriptError::new(NOT_IMPLEMENTED, "silkscreen blocks (M1, in progress)", block.location.clone()))
        }
        Some(_) => unreachable!(),
    }
}

/// `design_document`: the artwork intent written into the compiled IR.
pub fn design_document(design: &SilkscreenDesign) -> copper_json::Value {
    use copper_json::{Object, Value};
    let mark = &design.branding;
    let position = match mark.position {
        None => Value::Null,
        Some((x, y)) => {
            Value::Object(Object::from([("x_nm".to_owned(), Value::from(x)), ("y_nm".to_owned(), Value::from(y))]))
        }
    };
    let branding = Object::from([
        ("enabled".to_owned(), Value::Bool(mark.enabled)),
        ("position".to_owned(), position),
        ("size_nm".to_owned(), Value::from(mark.size_nm)),
        ("rotation_degrees".to_owned(), Value::from(mark.rotation_degrees.to_sci_string())),
        ("side".to_owned(), Value::from(mark.side)),
        ("version".to_owned(), Value::from(mark.version.as_str())),
        ("material".to_owned(), Value::from(mark.material)),
        ("exposed".to_owned(), Value::Bool(mark.exposed)),
        ("layout".to_owned(), Value::from(mark.layout.as_str())),
    ]);
    Value::Object(Object::from([
        ("branding".to_owned(), Value::Object(branding)),
        ("artwork".to_owned(), Value::Array(Vec::new())),
    ]))
}

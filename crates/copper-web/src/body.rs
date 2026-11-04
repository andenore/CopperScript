//! Fallback 3D bodies for the browser: the compiler's generator behind a JSON
//! boundary, so a viewer draws the same body KiCad export writes.
//!
//! Input (`copperscript.body-input/v1`) is one footprint in its own frame,
//! millimetres with y down as in a `.kicad_mod`:
//! `{name, height_mm?, pads: [{number, x_mm, y_mm, w_mm, h_mm, rotation_deg?,
//! kind: "smd"|"thru_hole"|"np_thru_hole", drill_mm?: [w, h]}],
//! fab_mm?: [x0, y0, x1, y1], courtyard_mm?: [x0, y0, x1, y1]}`.
//! Output (`copperscript.body/v1`) is `{kind, approximate, hash, wrl}`, the VRML
//! being millimetres in KiCad's model axes, or `{error}`.

use copper_json::{Object, Value, parse};
use copper_physical::MAX_COORDINATE;
use copper_physical::body::generate_body;
use copper_physical::footprint::{
    FootprintGraphic, FootprintLayer, FootprintPad, PadKind, PadShape, PhysicalFootprint, Size,
};
use copper_physical::geometry::Point;
use copper_units::Decimal;
use indexmap::IndexMap;

pub const INPUT_SCHEMA: &str = "copperscript.body-input/v1";
pub const OUTPUT_SCHEMA: &str = "copperscript.body/v1";
pub const MAX_INPUT_BYTES: usize = 1 << 20;
const MAX_PADS: usize = 4096;

fn number(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Float(number) => *number,
        Value::Int(integer) => integer.to_string().parse().ok()?,
        _ => return None,
    };
    number.is_finite().then_some(number)
}

fn nm(value: Option<&Value>, what: &str) -> Result<i64, String> {
    let mm = number(value).ok_or_else(|| format!("{what} must be a number"))?;
    let nm = (mm * 1e6).round();
    if nm.abs() > MAX_COORDINATE as f64 {
        return Err(format!("{what} is out of range"));
    }
    Ok(nm as i64)
}

fn positive(value: Option<&Value>, what: &str) -> Result<i64, String> {
    match nm(value, what)? {
        size if size > 0 => Ok(size),
        _ => Err(format!("{what} must be positive")),
    }
}

fn pair(value: Option<&Value>, what: &str) -> Result<Option<[Value; 2]>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) if items.len() == 2 => Ok(Some([items[0].clone(), items[1].clone()])),
        Some(_) => Err(format!("{what} must be two numbers")),
    }
}

fn rectangle(value: Option<&Value>, what: &str, layer: FootprintLayer) -> Result<Option<FootprintGraphic>, String> {
    let items = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(items)) if items.len() == 4 => items,
        Some(_) => return Err(format!("{what} must be [x0, y0, x1, y1]")),
    };
    let (start, end) = (
        Point::new(nm(items.first(), what)?, nm(items.get(1), what)?),
        Point::new(nm(items.get(2), what)?, nm(items.get(3), what)?),
    );
    if start.x_nm == end.x_nm || start.y_nm == end.y_nm {
        return Err(format!("{what} must have an area"));
    }
    Ok(Some(FootprintGraphic::Rectangle { start, end, width_nm: 100_000, layer, filled: false }))
}

fn pad(value: &Value) -> Result<FootprintPad, String> {
    let get = |key: &str| value.get(key);
    let size = Size::new(positive(get("w_mm"), "pad w_mm")?, positive(get("h_mm"), "pad h_mm")?)?;
    let kind = match get("kind").and_then(Value::as_str) {
        Some("smd") => PadKind::Smd,
        Some("thru_hole") => PadKind::ThroughHole,
        Some("np_thru_hole") => PadKind::NonPlatedThroughHole,
        _ => return Err("pad kind must be smd, thru_hole or np_thru_hole".to_owned()),
    };
    let drill = match (kind, pair(get("drill_mm"), "pad drill_mm")?) {
        (PadKind::Smd, _) => None,
        (_, Some([w, h])) => Some(Size::new(positive(Some(&w), "pad drill")?, positive(Some(&h), "pad drill")?)?),
        (PadKind::ThroughHole, None) => {
            let side = size.width_nm.min(size.height_nm) / 2;
            Some(Size::new(side.max(1), side.max(1))?)
        }
        (_, None) => Some(size),
    };
    // A drill that does not fit its pad is clamped: this only sizes a body.
    let drill = drill.map(|d| {
        let fit = |drill: i64, pad: i64| {
            if kind == PadKind::ThroughHole { drill.min(pad - 1).max(1) } else { drill.min(pad) }
        };
        Size { width_nm: fit(d.width_nm, size.width_nm), height_nm: fit(d.height_nm, size.height_nm) }
    });
    let rotation = number(get("rotation_deg")).unwrap_or(0.0).rem_euclid(360.0);
    Ok(FootprintPad {
        number: get("number").and_then(Value::as_str).unwrap_or_default().to_owned(),
        position: Point::new(nm(get("x_mm"), "pad x_mm")?, nm(get("y_mm"), "pad y_mm")?),
        size,
        kind,
        shape: PadShape::Rectangle,
        rotation_degrees: Decimal::parse(&format!("{}", rotation.round())).map_err(|e| e.to_string())?,
        drill,
        roundrect_ratio_ppm: 0,
        has_solder_mask: true,
        has_solder_paste: kind == PadKind::Smd,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    })
}

fn footprint(input: &Value) -> Result<PhysicalFootprint, String> {
    if input.get("schema").and_then(Value::as_str) != Some(INPUT_SCHEMA) {
        return Err(format!("expected schema {INPUT_SCHEMA}"));
    }
    let name = input.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).ok_or("name is required")?;
    let pads = input.get("pads").and_then(Value::as_array).ok_or("pads must be a list")?;
    if pads.len() > MAX_PADS {
        return Err(format!("at most {MAX_PADS} pads are accepted"));
    }
    let pads = pads.iter().map(pad).collect::<Result<Vec<_>, _>>()?;
    let mut graphics = Vec::new();
    graphics.extend(rectangle(input.get("fab_mm"), "fab_mm", FootprintLayer::Fabrication)?);
    graphics.extend(rectangle(input.get("courtyard_mm"), "courtyard_mm", FootprintLayer::Courtyard)?);
    let height_nm = match input.get("height_mm") {
        None | Some(Value::Null) => None,
        value => Some(positive(value, "height_mm")?),
    };
    PhysicalFootprint {
        name: name.to_owned(),
        body_size: Size::new(1_000_000, 1_000_000)?,
        pads,
        source_library_id: None,
        graphics,
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    }
    .validated()
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(Object::from_iter(entries.map(|(key, value)| (key.to_owned(), value))))
}

/// The generated body for one footprint description, or `{error}`.
pub fn body_document(input: &str) -> Value {
    let failure = |message: String| object([("schema", Value::from(OUTPUT_SCHEMA)), ("error", Value::from(message))]);
    if input.len() > MAX_INPUT_BYTES {
        return failure("input is too large".to_owned());
    }
    let parsed = match parse(input) {
        Ok(parsed) => parsed,
        Err(error) => return failure(format!("invalid JSON: {error}")),
    };
    match footprint(&parsed) {
        Ok(footprint) => {
            let body = generate_body(&footprint);
            object([
                ("schema", Value::from(OUTPUT_SCHEMA)),
                ("kind", Value::from(body.kind.value())),
                ("approximate", Value::Bool(body.approximate)),
                ("hash", Value::from(body.hash.clone())),
                ("wrl", Value::from(body.to_wrl())),
            ])
        }
        Err(message) => failure(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOIC8: &str = r#"{"schema":"copperscript.body-input/v1","name":"SOIC-8_3.9x4.9mm_P1.27mm",
      "pads":[{"number":"1","x_mm":-2.475,"y_mm":-1.905,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"2","x_mm":-2.475,"y_mm":-0.635,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"3","x_mm":-2.475,"y_mm":0.635,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"4","x_mm":-2.475,"y_mm":1.905,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"5","x_mm":2.475,"y_mm":1.905,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"6","x_mm":2.475,"y_mm":0.635,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"7","x_mm":2.475,"y_mm":-0.635,"w_mm":1.95,"h_mm":0.6,"kind":"smd"},
              {"number":"8","x_mm":2.475,"y_mm":-1.905,"w_mm":1.95,"h_mm":0.6,"kind":"smd"}],
      "fab_mm":[-1.95,-2.45,1.95,2.45]}"#;

    fn text<'a>(value: &'a Value, key: &str) -> &'a str {
        value.get(key).and_then(Value::as_str).unwrap_or_else(|| panic!("{key} in {value:?}"))
    }

    #[test]
    fn a_soic8_description_gives_a_leaded_body() {
        let body = body_document(SOIC8);
        assert_eq!(text(&body, "schema"), OUTPUT_SCHEMA);
        assert_eq!(text(&body, "kind"), "leaded");
        assert_eq!(body.get("approximate"), Some(&Value::Bool(true)));
        assert!(text(&body, "wrl").starts_with("#VRML V2.0 utf8"));
        assert_eq!(text(&body, "hash").len(), 64);
        assert!(text(&body, "wrl").contains("DEF leads"));
    }

    #[test]
    fn output_is_deterministic_and_follows_the_height() {
        let (a, b) = (body_document(SOIC8), body_document(SOIC8));
        assert_eq!(text(&a, "hash"), text(&b, "hash"));
        let taller = SOIC8.replace("\"name\"", "\"height_mm\":2.0,\"name\"");
        assert_ne!(text(&a, "hash"), text(&body_document(&taller), "hash"));
    }

    #[test]
    fn through_hole_pads_need_no_drill_and_headers_are_recognised() {
        let pads: Vec<String> = (0..3)
            .map(|i| {
                format!(
                    r#"{{"number":"{}","x_mm":0,"y_mm":{},"w_mm":1.7,"h_mm":1.7,"kind":"thru_hole"}}"#,
                    i + 1,
                    f64::from(i) * 2.54
                )
            })
            .collect();
        let input =
            format!(r#"{{"schema":"{INPUT_SCHEMA}","name":"PinHeader_1x03_P2.54mm","pads":[{}]}}"#, pads.join(","));
        assert_eq!(text(&body_document(&input), "kind"), "pin_header");
    }

    #[test]
    fn bad_input_is_an_error_document_not_a_panic() {
        for input in [
            "not json",
            r#"{"schema":"x","name":"a","pads":[]}"#,
            r#"{"schema":"copperscript.body-input/v1","pads":[]}"#,
            r#"{"schema":"copperscript.body-input/v1","name":"a","pads":[{"w_mm":-1,"h_mm":1,"kind":"smd","x_mm":0,"y_mm":0}]}"#,
            r#"{"schema":"copperscript.body-input/v1","name":"a","pads":[{"w_mm":1,"h_mm":1,"kind":"weird","x_mm":0,"y_mm":0}]}"#,
            r#"{"schema":"copperscript.body-input/v1","name":"a","pads":[],"fab_mm":[0,0,0,1]}"#,
            r#"{"schema":"copperscript.body-input/v1","name":"a","pads":[{"w_mm":1,"h_mm":1,"kind":"smd","x_mm":1e30,"y_mm":0}]}"#,
        ] {
            let body = body_document(input);
            assert!(body.get("error").is_some(), "{input} gave {body:?}");
            assert!(body.get("wrl").is_none());
        }
        assert!(body_document(&"x".repeat(MAX_INPUT_BYTES + 1)).get("error").is_some());
    }

    #[test]
    fn a_footprint_with_no_pads_still_gets_a_block() {
        let input = r#"{"schema":"copperscript.body-input/v1","name":"Mount","pads":[],"courtyard_mm":[-2,-2,2,2]}"#;
        assert_eq!(text(&body_document(input), "kind"), "block");
    }
}

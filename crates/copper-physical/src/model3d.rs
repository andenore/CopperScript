//! Footprint 3D model references (`(model ...)` of a `.kicad_mod`).
//!
//! A reference is carried on the footprint's `metadata` under
//! [`MODELS_KEY`], so the state codecs round-trip it without a format change.
//! Paths are kept as written; an unresolved `${KICAD10_3DMODEL_DIR}` prefix is
//! a reference, not an error.

use crate::footprint::PhysicalFootprint;

/// The metadata key holding the encoded references.
pub const MODELS_KEY: &str = "models_3d";
/// Footprint metadata key set (to `true`) when its model came from a part's
/// URL and may not be redistributed: exports keep it out of shared packages.
pub const LOCAL_ONLY_MODEL_KEY: &str = "models_3d_local_only";

/// One `(model ...)`: a file and its placement in the footprint frame
/// (millimetres and degrees, KiCad's model-frame axes).
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintModel {
    pub path: String,
    pub offset_mm: [f64; 3],
    pub scale: [f64; 3],
    pub rotate_degrees: [f64; 3],
}

impl FootprintModel {
    pub fn new(path: impl Into<String>) -> Self {
        FootprintModel { path: path.into(), offset_mm: [0.0; 3], scale: [1.0; 3], rotate_degrees: [0.0; 3] }
    }

    /// Whether the path can be stored: it is one line without tabs.
    pub fn storable(path: &str) -> bool {
        !path.is_empty() && !path.contains(['\t', '\n', '\r'])
    }

    /// The path extension, lower-cased (`step`, `stp`, `wrl`, ...).
    pub fn extension(&self) -> Option<String> {
        let name = self.path.rsplit(['/', '\\']).next()?;
        let (_, extension) = name.rsplit_once('.')?;
        Some(extension.to_ascii_lowercase())
    }
}

fn triple(values: &[f64; 3]) -> String {
    format!("{} {} {}", values[0], values[1], values[2])
}

fn parse_triple(text: &str) -> Option<[f64; 3]> {
    let mut parts = text.split(' ').map(str::parse::<f64>);
    let value = [parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?];
    (parts.next().is_none() && value.iter().all(|v| v.is_finite())).then_some(value)
}

/// One line per model: `path TAB offset TAB scale TAB rotate`.
pub fn encode_models(models: &[FootprintModel]) -> String {
    models
        .iter()
        .map(|m| format!("{}\t{}\t{}\t{}", m.path, triple(&m.offset_mm), triple(&m.scale), triple(&m.rotate_degrees)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The inverse of [`encode_models`]; malformed lines are skipped.
pub fn decode_models(text: &str) -> Vec<FootprintModel> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let path = fields.next().filter(|path| !path.is_empty())?;
            let offset_mm = parse_triple(fields.next()?)?;
            let scale = parse_triple(fields.next()?)?;
            let rotate_degrees = parse_triple(fields.next()?)?;
            fields.next().is_none().then(|| FootprintModel { path: path.to_owned(), offset_mm, scale, rotate_degrees })
        })
        .collect()
}

/// The 3D models a footprint references, in file order.
pub fn footprint_models(footprint: &PhysicalFootprint) -> Vec<FootprintModel> {
    footprint.metadata.get(MODELS_KEY).map(|text| decode_models(text)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_round_trip_through_metadata_text() {
        let models = vec![
            FootprintModel {
                path: "${KICAD10_3DMODEL_DIR}/Package_SO.3dshapes/SOIC-8 x.step".to_owned(),
                offset_mm: [0.0, -1.5, 0.25],
                scale: [1.0, 1.0, 1.0],
                rotate_degrees: [0.0, 0.0, 90.0],
            },
            FootprintModel::new("local.wrl"),
        ];
        assert_eq!(decode_models(&encode_models(&models)), models);
        assert_eq!(models[0].extension().as_deref(), Some("step"));
    }

    #[test]
    fn malformed_lines_are_skipped() {
        assert!(decode_models("a.step\t0 0\t1 1 1\t0 0 0").is_empty());
        assert!(!FootprintModel::storable("a\tb.step"));
    }
}

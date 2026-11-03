use copper_physical::kicad_mod::parse_kicad_mod;
use copper_physical::model3d::{FootprintModel, footprint_models};

fn footprint(model: &str) -> String {
    format!(
        "(footprint \"R_0402\" (version 20240108) (generator \"test\") (layer \"F.Cu\")\n\
         (attr smd)\n\
         (pad \"1\" smd rect (at -0.48 0) (size 0.56 0.62) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n\
         (pad \"2\" smd rect (at 0.48 0) (size 0.56 0.62) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n\
         {model})"
    )
}

#[test]
fn kicad_10_model_is_kept_not_warned_about() {
    let text = footprint(
        "(model \"${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0402_1005Metric.step\"\n\
         (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90)))",
    );
    let imported = parse_kicad_mod(&text, "r.kicad_mod", true).expect("strict import accepts a model");
    assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
    let models = footprint_models(&imported.footprint);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].rotate_degrees, [0.0, 0.0, 90.0]);
    assert_eq!(models[0].extension().as_deref(), Some("step"));
}

#[test]
fn legacy_at_offset_is_inches() {
    let text = footprint("(model r.wrl (at (xyz 0.1 0 0)) (scale (xyz 0.3937 0.3937 0.3937)) (rotate (xyz 0 0 0)))");
    let models = footprint_models(&parse_kicad_mod(&text, "r.kicad_mod", false).unwrap().footprint);
    assert_eq!(models.len(), 1);
    assert!((models[0].offset_mm[0] - 2.54).abs() < 1e-9);
    assert_eq!(models[0].scale, [0.3937; 3]);
}

#[test]
fn a_model_without_placement_uses_identity() {
    let text = footprint("(model \"a.step\")");
    let models = footprint_models(&parse_kicad_mod(&text, "r.kicad_mod", false).unwrap().footprint);
    assert_eq!(models, vec![FootprintModel::new("a.step")]);
}

#[test]
fn an_unusable_model_still_warns() {
    let text = footprint("(model \"a.step\" (offset (xyz x 0 0)))");
    let imported = parse_kicad_mod(&text, "r.kicad_mod", false).unwrap();
    assert!(imported.warnings.iter().any(|w| w.contains("3D model")));
    assert!(footprint_models(&imported.footprint).is_empty());
}

#[test]
fn no_model_means_no_metadata() {
    let imported = parse_kicad_mod(&footprint(""), "r.kicad_mod", false).unwrap();
    assert!(!imported.footprint.metadata.contains_key("models_3d"));
}

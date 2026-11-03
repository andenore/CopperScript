# 3D models

Status: the data path and the fallback body exist in the compiler. KiCad export
of models and the browser 3D view are not built yet (see *Not yet*).

## Model references

A `.kicad_mod` `(model "path" (offset (xyz ..)) (scale (xyz ..)) (rotate (xyz ..)))`
is imported, not ignored. The legacy `(at (xyz ..))` form is read as inches.
References are stored on the imported footprint's `metadata` under `models_3d`
(one line per model: path, offset in mm, scale, rotation in degrees), so the
state files round-trip them with no format change. Read them with
`copper_physical::model3d::footprint_models`.

Paths are kept as written. An unresolved `${KICAD10_3DMODEL_DIR}/...` prefix is
a reference, not an error. A `(model ...)` with a malformed placement still
produces the `ignored 3D model reference` warning.

## Part fields

```text
part Widget {
    model = "models/widget.step";   // file path, relative to the part file
    height = 1.25 mm;               // positive typed length
    ...
}
```

Both are optional. They are stored on `PartDefinition.metadata` as `model_3d`
(the path) and `height_nm` (exact nanometres). A bad `height` is `CMP209` and a
non-string `model` is `CMP208`. Resolving `model` through the package lock is
not wired yet.

## Fallback body

`copper_physical::body::generate_body(&footprint)` returns an approximate,
deterministic body when no model file exists. It reads only footprint data: pad
geometry, the fabrication and courtyard outlines, the footprint name and
`height_nm`. Recognised kinds:

| Kind | Rule | Body |
|---|---|---|
| `chip` | two SMD pads | EIA 0201-1210 envelope from the name, else estimated from the pads; end caps |
| `leaded` | SMD pads mostly outside the fabrication outline | fabrication outline extruded 0.1 mm off the board, plus gull-wing lead blocks |
| `no_lead` | SMD pads inside the outline (QFN, DFN) | fabrication outline extruded |
| `pin_header` | through-hole pads on a 1-5 mm grid | block of pitch squares, 0.25-pitch square pins from -3 mm to 6 mm above the body |
| `block` | anything else | fabrication, else courtyard, else pad outline extruded 1 mm |

Heights come from `height_nm` first, then the family named in the footprint
(SOIC 1.75 mm, TQFP/LQFP 1.4, TSSOP/MSOP/SOT 1.1, QFN/DFN 0.9), then 1.5 mm
leaded or 0.9 mm otherwise. The body is **always** `approximate`; it is for fit
and clearance review, not a manufacturer model.

The mesh is millimetres in KiCad's model axes (`y = -footprint_y`, z up from the
board surface), so it needs no transform. `GeneratedBody::to_wrl()` writes VRML
2.0; `hash` is the SHA-256 of the generator version and that text, so any change
to the inputs or the generator changes it.

## Not yet

- KiCad export copying models into a `3dshapes` folder and writing `(model ...)`.
  The VRML unit convention KiCad expects (1 mm or 1/2.54 mm per unit) is
  unverified; check it against KiCad before writing generated files.
- Resolving `model` and `(model ...)` paths through the package lock and a
  remote 3D module (`kicad-packages3D`, which needs a sparse fetch).
- Applying a part's `height` to the footprint it selects.
- `copper-web` mesh export and the CopperReview WebGL view.
- STEP output for generated bodies.

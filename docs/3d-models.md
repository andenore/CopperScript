# 3D models

Status: model references, part `model`/`height`, the fallback body and KiCad
export exist in the compiler, and CopperReview draws the exported bodies. KiCad's
3D library resolves per file through the lock. STEP output for generated bodies
is not built (see *Not yet*).

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
(the path as written), `model_3d_source` (the declaring file) and `height_nm`
(exact nanometres). A bad `height` is `CMP209` and a non-string `model` is
`CMP208`.

When a board is physicalized the part's values override its footprint's: the
footprint gets that `height_nm` (which also feeds the generated body and the
`CopperScriptHeightMM` property) and that model. The footprint is renamed with a
`__3d_<digest>` suffix, so two parts can share one source footprint and differ
in 3D. A `model` that is not a file relative to the part file is reported as an
import warning (`... 3D model ... not found; using a generated body`) and
ignored. So is one that resolves outside the part file's directory (`../`,
an absolute path or a symlink out): the model is copied into the
manufacturing package, so a dependency must not name other files on disk.
Only `.step`, `.stp`, `.wrl` and `.vrml` files are copied.

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

## KiCad's 3D library

`kicad-packages3D` is several GB, so it is never cloned or inventoried whole. A
project requires it like any module:

```text
require github.com/KiCad/kicad-packages3D 10.0.8
replace github.com/KiCad/kicad-packages3D => gitlab.com/kicad/libraries/kicad-packages3D 10.0.8
```

The compiler treats a module named `.../kicad-packages3D` as *sparse*. A model
is read with `git show REV:PATH` from a blobless shallow clone kept in
`.copper-cache` (about 300 KB of tree objects; git fetches only the blobs asked
for), and `copper.lock` gains one entry per model used: its path, size and
SHA-256, under the module's `sparse-git:` source. `copper lock` skips the module,
a locked build refuses a model that is not recorded (`PKG008`, run `copper lock`)
or whose bytes changed, and `--offline` uses the cache.

Two things name a model from the library:

- A footprint's own `(model "${KICADn_3DMODEL_DIR}/Lib.3dshapes/X.step")`, which
  is what footprints from `kicad-footprints` carry. With the module required it
  resolves to the locked file; without it the reference is left as written.
- A part's `model = "github.com/KiCad/kicad-packages3D/Lib.3dshapes/X.step";`,
  which overrides the footprint's.

A model the library does not have at that revision, or that cannot be fetched
(for example offline with a cold cache), is dropped with a warning and the
generated body stands in. A lock mismatch is always an error. The export copies
resolved files into `CopperScript.3dshapes`, so the project opens in KiCad
without a KiCad install. Part `model` URLs need resolved footprints; with
`--allow-proxy-footprints` they warn and fall back to the generated body.

## KiCad export

`export-kicad-pcb` writes a `(model ...)` into every exported footprint, in the
board and in `CopperScript.pretty`:

1. A model that is a local file (a part's `model`) is copied to
   `CopperScript.3dshapes/<digest>_<name>.<ext>` and referenced through
   `${KIPRJMOD}`. Files are copied as text, which STEP and VRML are.
2. A footprint whose source carries `(model ...)` keeps those references
   unchanged (`${KICAD10_3DMODEL_DIR}/...` resolves in a stock KiCad install).
3. Any other footprint gets its generated body, referenced as
   `${KIPRJMOD}/CopperScript.3dshapes/<item>.wrl`.

The generated body is written for every footprint, and a hidden
`CopperScriptBody` property names it, so viewers that cannot read STEP still
have a body. The export lists the footprints that rely on a generated body in
KiCad as a warning. The VRML
files are in **millimetres**. KiCad reads one VRML unit as 0.1 inch (2.54 mm), so
generated references carry `(scale (xyz 0.3937... ))` (`KICAD_WRL_SCALE`). This
was checked by rendering generated 0805 and 0402 bodies with KiCad 10's
`kicad-cli pcb render`: with scale 1 they came out 2.54 times too large; with
the scale they sit inside their pads.

## Not yet

- Locking a part's *local* model file. It is read at export time and is not part
  of `copper.lock` unless it sits in a locked module.
- Drawing KiCad's STEP models outside KiCad. CopperReview reads VRML only, and the
  KiCad library ships STEP only, so it shows the generated body for those parts.
- STEP output for generated bodies (VRML only).
- Non-zero model rotation, model offsets and back-side parts in the CopperReview
  view are placed best-effort and have not been compared with KiCad.

## Viewing

CopperReview draws the exported `.wrl` files (`copper-review BOARD.kicad_pcb`,
then the **3D** button); see its README. The viewer reads the compiler's own
bodies rather than regenerating them, so KiCad and the viewer agree.

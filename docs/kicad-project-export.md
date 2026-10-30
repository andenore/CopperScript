# Generated, self-contained KiCad PCB projects

Every PCB-producing command writes the complete project alongside its output:
`export-kicad-pcb`, `plan-layout`, `route-global --pcb-output`, `route-board`,
critical preflight and manufacturing export. KiCad filled-plane verification
uses the same complete export in its private temporary directory.

```text
build/my-board/
├── board.kicad_pcb
├── board.kicad_pro
├── fp-lib-table
└── CopperScript.pretty/
    └── <readable-name>__<geometry-digest>.kicad_mod
```

CopperScript generates all of these files. Users maintain `.copper` source and
upstream source libraries such as CopperLib, not the generated KiCad library.
No new options, downloads, KiCad Python runtime or manual library setup are
required to export them. The output parent directory is created automatically.
The `.kicad_pro` stem follows a custom output filename, including its metadata.

## Resolution versus export

The existing dependency and footprint resolvers obtain the inputs. Explicit
footprint roots and direct `.kicad_mod` paths continue to work, including locked
offline builds. The exporter reads **only the physical IR already produced by
those resolvers**; it does not fetch URLs or reread the original footprint files.

Only footprint types used by placed components are exported, once per type.
Footprint names combine a sanitized readable name and a geometry/source-identity
digest. Different upstream namespaces, names that sanitize alike and changed
geometry remain distinct. Machine-local source cache paths are excluded from
asset identity. Canonical library content is independent of board name and
component placement. Different revisions can coexist in one output directory
without replacing each other's geometry.

The physical IR's original library identifiers, source hashes, rigid-template
bindings and connectivity are unchanged. Only the KiCad artifact references
become `CopperScript:<generated-name>`.

## What the library table does

`fp-lib-table` maps the `CopperScript` library nickname to
`${KIPRJMOD}/CopperScript.pretty`. `KIPRJMOD` resolves relative to the KiCad
project, so the directory can be moved or shared without absolute paths or
changes to the machine's global library table. Keep the **entire export
directory**, not just the PCB, when opening it elsewhere.

The board embeds its footprint instances and the generated library contains
their canonical front-side, unrotated, net-free definitions. Both use the same
geometry renderer. This lets KiCad compare instances with the exact exported
library rather than unrelated installed library versions.

## Safety and verification

- Repeated exports regenerate their owned files deterministically. Unused old
  digest-named footprints are not deleted: another board in the directory may
  still reference them.
- An existing `fp-lib-table` with different content is **not overwritten or
  automatically merged**. Export to a separate build directory instead.
- Manifest paths must stay inside the output directory; traversal, absolute
  artifact paths, duplicate destinations and escaping symlinks fail before
  export writes any files.
- Manufacturing inventories/checksums include the local library and table.
  Filled-plane verification binds its export digest to every generated asset,
  not just the PCB and project file. Old export evidence must be regenerated.
- Missing-library and mismatch warnings are not suppressed or waived. Optional
  independent KiCad tests check rotated front/back instances, relocated projects
  with spaces in their paths and actual keepout enforcement.

Project export writes the authored drilled-hole spacing to KiCad's
`min_hole_to_hole` setting. KiCad treats `min_hole_clearance` as a separate
copper-to-hole setting; its established export behavior is retained.
These keys are distinct in [KiCad's setting definitions](https://docs.kicad.org/doxygen/board__design__settings_8cpp_source.html).
Changing project rules invalidates prior native evidence even when the
board copper bytes are identical.

The full-vertical six-layer **unrouted placement** was checked with KiCad
10.0.6: zero non-connectivity violations, including zero library findings;
204 unconnected items remain expected until routing is completed.

This does not certify source footprints or RF/manufacturing performance.
Existing importer warnings about omitted user text, 3D models and unsupported
presentation items remain visible. A generated library that matches the board
is not proof of equivalence to every feature of the upstream original.

## Schematic review exports

`export-kicad` also writes a self-contained symbol library: a content-addressed
`CopperScript_<digest>.kicad_sym`, `sym-lib-table`, and a same-stem
`.kicad_pro` alongside the schematic. The embedded and external symbols come
from the same renderer. KiCad needs the project file to resolve its local
library table; an existing project is preserved so schematic export cannot
replace PCB rules or user settings. Different existing library/table contents
are rejected before writing. Use a fresh export directory when inputs change.

The library identity is based on its complete rendered content. Library-table
paths use `${KIPRJMOD}` and native tests verify connectivity and symbol-library
matching after relocation, including paths containing spaces. This resolves
missing symbol-library diagnostics; it does not suppress electrical ERC,
footprint-link warnings or the current flat single-sheet layout limitation.

## PCB coordinate transforms

Footprint-owned keepouts remain nested under their owner, but their polygon
coordinates in a KiCad **board file** must be board coordinates. Export now
applies the same translation/rotation/side transform as native physical DRC.

Back-side instances also explicitly mirror pads, graphics and text. KiCad's
canonical back-side convention reflects local Y and adds 180 degrees to the
IR's placement angle, reproducing the IR's local-X reflection without changing
world-space copper. Footprint library matching is checked after normalization,
and keepout tests verify enforcement at translated/rotated physical positions.
See [KiCad's footprint flip implementation](https://github.com/KiCad/kicad-source-mirror/blob/10.0/pcbnew/footprint.cpp)
for the target's normalization convention.

## Rust API

```rust
let manifest = copper_pcb::kicad_pcb::generate(&physical_board)?;
copper_pcb::project::write_kicad_project(&manifest, Path::new("build/my-board/board.kicad_pcb"))?;
```

`generate` remains a pure artifact producer. Call the writer rather than saving
only `manifest.artifacts[0]` if you need a portable, independently checked project.

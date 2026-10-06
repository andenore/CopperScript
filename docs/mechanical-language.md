# Mechanical source specification

Status: accepted and implemented (unreleased v0.1).

Named datums, stable straight boundary edges and component/pad/mating-face
attachments are supported; see [advanced mechanical intent](advanced-mechanical-intent.md).
That specification also defines component-owned body overhang, declared height,
side-specific enclosure limits and moving/rotating tool-access regions. None of
these mechanical policies relax copper DRC or alter the manufacturing outline.
`examples/mechanical_anchors/board.copper` demonstrates a rear 45-degree pad anchor
and an edge-relative mating-face point. Open it with
`make EXAMPLE=mechanical-anchors edit`. Attachments own the resolved pose:
edit the attachment or its datum rather than adding a competing placement lock.

Electrical `Board` and reusable electrical modules contain no outline geometry.
A compiled `Design` combines an electrical `Board` with an optional, separate
`MechanicalDesign`. Physicalization consumes that aggregate. Electrical-only
APIs remain useful for ERC, schematic export and simulation; they validate but
do not return mechanical data. JSON compilation emits mechanical data as a
sibling of electrical fields, never as a component or connectivity constraint.

Only a top-level board may contain one `mechanical` block. It may apply imported
[`board_profile` definitions](mechanical-profiles.md) and add local features;
the combined result requires exactly one outline. Coordinates and dimensions are explicit typed lengths, converted
exactly to integer nanometres; negative coordinates are legal, sub-nanometre
precision is rejected. Unknown/duplicate properties and invalid topology fail
with source locations. No implicit healing or geometry inference is allowed.

```copper
mechanical {
    outline circle { center = (25mm, 25mm); diameter = 50mm; }
    cutout window polygon {
        vertices = [(20mm, 20mm), (23mm, 20mm), (23mm, 23mm), (20mm, 23mm)];
    }
    hole H1 { position = (10mm, 25mm); diameter = 3mm; head_clearance_radius = 3mm; }
    rules { minimum_track_width = 0.15mm; minimum_clearance = 0.15mm; }
}
```

Alternative outlines:

```copper
outline rectangle { width = 60mm; height = 40mm; origin = (-5mm, 0mm); }
outline polygon { vertices = [(0mm, 0mm), (40mm, 0mm), (40mm, 20mm), (0mm, 30mm)]; }
```

Rectangle origin defaults to `(0mm, 0mm)`. Circle center defaults to
`(diameter/2, diameter/2)`; optional `maximum_chord_error` defaults to `0.01mm`.
Circles remain exact circles for material queries and KiCad `Edge.Cuts` export;
the bounded inscribed ring is only a conservative query representation.
Polygon closure is implicit. One connected simple outer boundary is supported.
Cutouts must be simple polygonal voids strictly inside material, without
touching, overlap or nesting. Holes are board-owned round NPTH, not BOM parts.
Their optional head-clearance radius must contain the drill. Drills and head
clearances must fit material and satisfy the existing hole topology checks.

Optional rules override physicalizer defaults, not electrical ERC. Allowed
length properties are `minimum_clearance`, `minimum_hole_clearance`,
`minimum_track_width`, `default_track_width`, `default_via_size`,
`default_via_drill`, `minimum_slot_width`. Omitted values keep the selected fabrication profile's
defaults. This is design intent, not manufacturing qualification.

`minimum_hole_clearance` applies to all copper near every non-plated hole. A
vendor land pattern whose pads sit closer to the part's own holes takes a
documented `constraint hole_clearance(J1) { clearance = ...; reason = ...; }`
instead of a lower board rule. It relaxes only that footprint's pads against
that footprint's own holes; tracks, vias, other components and board-owned
holes and slots keep `minimum_hole_clearance`. See
[the language reference](language-reference.md#constraints).

A source outline is authoritative. CLI `--width-mm` / `--height-mm` are a
rectangle fallback only when the source has no mechanical block. Inspection
grid placement uses the source bounding box; real placement legality still
uses actual material, not its bounding box. Source fixed-placement constraints
are applied by generic physicalization, including side and rotation.
Fixed rotations may use arbitrary angles, including 45 degrees. An explicit
`allowed_orientations` constraint must still admit the fixed rotation; conflicting
constraints fail rather than depending on declaration order. An optional typed
`edge_clearance` on `fixed_placement` sets a physical courtyard margin, never a
copper-clearance waiver.

`copper_zone` inset supports circles and rectangles; polygon outlines support
zero inset only. Nonzero general polygon offsets remain unsupported rather
than producing an unsafe approximation. Zones exclude polygonal cutouts;
native refill handles NPTH clearances and must independently prove connectivity.
Insets intersecting a cutout are rejected until polygon-boolean clipping exists.

Example builders and fictitious library data live outside `pcbir`. Generic
compiler, placement, routing and backend modules must not import examples or
encode example references/paths. The round LED source owns its circle, fixed
ring placements, clearance rules and rear ground-pour intent.

Exact curved outlines and slots:

```copper
outline rounded_rectangle {
    width=40mm; height=30mm; corner_radius=3mm;
    maximum_chord_error=0.01mm;
}
slot S { start=(17mm,10mm); end=(23mm,10mm); width=1.5mm; }
```

Alternatively `outline path { maximum_chord_error=0.01mm; }` owns ordered
`boundary NAME line { start=(...); end=(...); }` and
`boundary NAME arc { start=(...); mid=(...); end=(...); }` declarations.
Endpoints must join exactly and close; no healing is performed. Line primitive
IDs are stable attachment edges; arc approximation chords are not edges.
Manufacturing retains three-point arcs. Shared placement/routing queries use a
bounded inscribed approximation. Initially curved paths must be convex with
arcs no greater than 180 degrees; all-line paths may be concave. Rounded corners
require a positive radius strictly below half the smaller dimension. Paths have
2–256 primitives and at most 4,096 query vertices; unsupported queries fail.

Slots are board-owned NPTH capsules; start/end name distinct cutter centres,
not the outside tips. They exclude bodies and copper from board material, obey
hole/slot proximity checks, and export native oval drills / Excellon G85. The
default generic minimum width is 1mm, configurable through mechanical rules;
this is not a supplier capability guarantee. A fabrication profile must specify
its milling capability separately. Gerber/drill inspection export is supported.
Complete closed paths can be reviewed in one atomic editor transaction, then
saved or undone byte-exactly. Imported profile geometry remains read-only.
See `make EXAMPLE=mechanical-curves route` for a real-footprint routing example.

Locked DXF/enclosure guides are separate from manufacturing boundaries:

```copper
reference CASE dxf {
    file="assets/case.dxf"; sha256="<64 lowercase hex digits>";
    units=mm; frame=cartesian; position=(0mm,30mm);
    rotation=0; mirror_x=false; side=front; purpose="Enclosure reference only";
}
```

The mandatory checksum pins exact bytes (keep DXF assets `-text` in Git
attributes). Paths stay beneath the declaring source file, including imported
profiles; URLs, parent traversal, symbolic links and junctions are rejected.
No browser file-read or upload API is provided. Units are `mm` or `inch`;
non-unitless DXF header units must agree. `cartesian` converts DXF Y-up to board
Y-down; `board` retains Y-down. Apply frame conversion, optional local X mirror,
counter-clockwise rotation, then board-frame translation. Defaults: zero pose,
no mirror, both sides, reference-only purpose. No independent scale is allowed.

Whitelist: planar LINE/CIRCLE/ARC and zero-width, zero-bulge LWPOLYLINE. Closed
polylines must be simple; open guide lines remain open. Unsupported geometry,
blocks, extrusion/elevation/thickness, malformed input or changed checksums fail
without healing. Per asset: 1MiB, 2,048 entities, 8,192 vertices; closed polylines
have at most 512 vertices for bounded topology checks.
Per design: 32 assets, 8,192 entities, 16,384 vertices. Guides are read-only
and side/visibility-filtered; only source-owned declarations' transforms are
editable through review/save/undo. Assets are checked again before save; changed
drawings visibly stale the guide and require source reload. They never become
board material, placement clearance, copper, drills or manufacturing output.
Use `enclosure`/keepouts for actual constraints; 3D inspection is deferred.
Example: `make EXAMPLE=mechanical-reference edit`.

Deferred: concave curved paths, curved cutouts, plated board-owned holes, multiple boards,
panelization, a general polygon-offset engine, and independent CAM qualification
of nonrectangular releases. Existing manufacturing gates remain closed for
unqualified mechanical geometry; exporting/routing is not production signoff.

## Board stack-up

A board's `mechanical` block may declare its stack-up once, ordered from top
to bottom. Copper layers and dielectrics alternate, starting and ending with
copper:

```copper
mechanical {
    outline rectangle { width = 60mm; height = 40mm; }
    stackup {
        copper F.Cu { thickness = 0.035mm; }
        dielectric P1 { thickness = 0.1mm; er = 4.1; loss_tangent = 0.02; material = "3313"; type = prepreg; }
        copper In1.Cu { thickness = 0.0175mm; }
        dielectric C1 { thickness = 0.55mm; er = 4.6; type = core; }
        copper In2.Cu { thickness = 0.0175mm; }
        dielectric P2 { thickness = 0.1mm; er = 4.1; }
        copper In3.Cu { thickness = 0.0175mm; }
        dielectric C2 { thickness = 0.55mm; er = 4.6; }
        copper In4.Cu { thickness = 0.0175mm; }
        dielectric P3 { thickness = 0.1mm; er = 4.1; material = "3313"; }
        copper B.Cu { thickness = 0.035mm; }
    }
}
```

- `copper NAME` names a KiCad copper layer: `F.Cu`, then `In1.Cu`, `In2.Cu`
  … in order, then `B.Cu`. Its only property is `thickness`.
- `dielectric NAME` requires `thickness` and the relative permittivity `er`
  (a unitless number of at least 1). `loss_tangent` (positive number),
  `material` (nonempty name) and `type` (`core` or `prepreg`) are optional.
  Names are unique across the stack-up.
- Thicknesses are positive typed lengths in exact integer nanometres.
- The total thickness becomes the board thickness (`Stackup.thickness_nm`).

Layer, order and property errors fail at compile time with source locations
(`MEC006`; an unknown layer kind is `PAR014`). The stack-up lowers to
`Stackup.physical_layers` during physicalization. The selected copper-layer
count (`--layers`) must name exactly the declared copper layers; otherwise
physicalization fails with the stack-up's location. A stack-up belongs to the
board, so `board_profile` definitions cannot declare one.

When a stack-up is declared, the KiCad PCB export writes it to the board
setup with KiCad's `dielectric 1`, `dielectric 2` … names. A dielectric
without a `type` follows KiCad's default construction: `core` on a two-layer
board, otherwise prepreg and core alternating from the top. Undeclared
material and loss tangent are left to KiCad's defaults. Impedance screening
(`copper si-check`, see the [language reference](language-reference.md#signal-integrity-screening))
uses the declared geometry. The stack-up is design intent and screening input;
the fabricator's stack-up and impedance qualification remain authoritative.

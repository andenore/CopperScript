# Mechanical source specification

Status: accepted and implemented (unreleased v0.1).

Named datums, stable straight boundary edges and component/pad/mating-face
attachments are supported; see [advanced mechanical intent](advanced-mechanical-intent.md).
`examples/mechanical_anchors.copper` demonstrates a rear 45-degree pad anchor
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
`default_via_drill`. Omitted values keep the selected fabrication profile's
defaults. This is design intent, not manufacturing qualification.

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

Deferred: curved polygon edges/slots, plated board-owned holes, multiple boards,
panelization, a general polygon-offset engine, and independent CAM qualification
of nonrectangular releases. Existing manufacturing gates remain closed for
unqualified mechanical geometry; exporting/routing is not production signoff.

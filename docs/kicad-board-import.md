# KiCad board import

`pcbir.importers.kicad_pcb.load_kicad_pcb(path)` reads a placed KiCad board
(`.kicad_pcb`, KiCad 5 to 10) and its adjacent `.kicad_pro` into physical IR:
board outline and cutouts, copper stackup, footprints and their poses, pad
nets, copper zones, rule-area keepouts and design rules. Tracks, vias, zone
fills, text and dimensions are counted in `BoardImportResult.dropped` and not
imported. The result is a placed, unrouted board for comparing routers on
third-party designs. It is not fabrication data.

## Exact parts

Each embedded footprint is converted to front-side library form and imported
by the strict `.kicad_mod` importer. The conversion inverts CopperScript's own
exporter: a back-side footprint's library geometry is its KiCad-local geometry
mirrored in Y with IR rotation `A - 180`, and pad angles stored in board
coordinates become local angles (`a - A` front, `A - a` back). Exporting a
CopperScript board and importing it reproduces every placement, pad position,
size and orientation, net, outline and stackup. KiCad 5 custom copper layer
names ("Top", "Inner1.Cu") are mapped by layer id; legacy centre-and-sweep
arcs are converted. A surface pad on the opposite copper side of its footprint
becomes a companion placement `REF~opposite` on that side at the same pose,
with identical copper.

Rules come from `.kicad_pro` net classes (KiCad 6 member lists, 7+
assignments, 8+ patterns) or KiCad 5 `net_class` blocks, otherwise KiCad's
defaults. Non-default classes become per-net width and clearance rules.

## Approximations

Board-only constructs the strict importer rejects are replaced by a superset
of their copper or dropped when they do not change copper: custom and
trapezoid pads become bounding rectangles, chamfers and pad-level clearance
overrides are dropped, plated pads listed on some layers are treated as on
all layers, copper graphics and zones on nets without pads become keepouts
for every net, and an outline that does not close falls back to its bounding
box. Each is counted in `BoardImportResult.approximations` and the board's
`import_approximations` metadata. Surface pads on both outer layers, and
footprints on non-copper layers, are rejected.

## Placement legality

Imported boards set `metadata["placement_legality"] = "external"`. The
placement under test was made by another tool, so physical DRC reports
`placement_legality` coverage as not applicable instead of applying the
planner's edge and courtyard conventions. Copper clearance, short, hole and
connectivity checks are unchanged. Route an imported board by passing its
placement as `initial_placement` to `run_routing_pipeline`, so the placer
does not move it.

On a 148-board sample of the public PCBench corpus (KiCad 5), 145 boards
import; the three failures use surface pads on both outer layers.

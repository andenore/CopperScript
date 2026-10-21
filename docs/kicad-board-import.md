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

KiCad's DRC enforces only the minimum track width (the project's
`min_track_width`, or `trace_min` in a KiCad 5 board); a class width is the
width new tracks start with, and real designs narrow at lands narrower
than the track. A class wider than both that minimum and the default width,
and at least as wide as the smallest land on one of its nets, gives that net
a neck-down region (`breakout_width` at the minimum, `breakout_length` twice
the class width and at least 1 mm): the router keeps the class width where
it fits and narrows only next to the net's lands. Nets whose lands are all
wider than their tracks keep one width, because neck-down regions cost
search time (2.6 times on a 1.25 mm supply net whose lands are all wider).

## Approximations

Board-only constructs the strict importer rejects are replaced by a superset
of their copper or dropped when they do not change copper: custom and
trapezoid pads become bounding rectangles (a surface custom pad's rectangle
is its copper's own box, which may sit off the anchor; a plated one stays
centred on its drill), chamfers and pad-level clearance
overrides are dropped, plated pads listed on some layers are treated as on
all layers, copper graphics, copper text and zones on nets without pads
become keepouts for every net, and an outline that does not close falls back to its bounding
box. Each is counted in `BoardImportResult.approximations` and the board's
`import_approximations` metadata. Surface pads on both outer layers, and
footprints on non-copper layers, are rejected.

Copper graphics and text have no net, so their keepouts are grown by the
largest net-class clearance: any routed net then keeps its own clearance
from them. Text uses its glyph box (each character advances 0.9 font
widths, 0.5 for narrow glyphs such as `I`, `1` and `.`, 1.15 for wide ones
such as `M` and `W`; lines are 1.62 heights apart; plus the stroke), rotated and justified
as in the file. Footprint text, whose stored angle differs between KiCad
versions, uses the square that holds the box at any rotation about its
anchor. Hidden text is not copper and is ignored.

A surface pad that straddles the board edge (a wire or contact pad milled
off at fabrication) is clipped, for routing only, to its copper inside the
outline: its centre may lie outside the board, where no access stem is
legal. A rectangular pad, square to the board axes, whose part inside the
outline is a rectangle, becomes that rectangle in a footprint variant of its
own placement (`<footprint>~clipped~<reference>`), counted as "pad clipped
to the board outline". Other straddling pads stay as they are ("pad
straddling the board edge left unclipped"). The written board keeps the
original footprints. D3-A board 0034 has two 5 × 10 mm wire pads on an
8 mm tall board, centred 0.5 mm outside it.

## Placement legality

Imported boards set `metadata["placement_legality"] = "external"`. The
placement under test was made by another tool, so physical DRC reports
`placement_legality` coverage as not applicable instead of applying the
planner's edge and courtyard conventions. Copper clearance, short, hole and
connectivity checks are unchanged. Route an imported board by passing its
placement as `initial_placement` to `run_routing_pipeline`, so the placer
does not move it.

Spacing findings that involve only placed pads (pad to pad, pad to board
edge, pad to keepout, between pad holes) are fixed by the imported
footprints and placement; no routing change can alter them. On these boards
they are reported as warning-level `DRC-INPUT-COPPER` findings naming the
original check, so they do not block package access or count as routing
errors. Often they come from the approximations above, or from the source
project's own relaxed rules (for example a copper-to-edge clearance below
the copper clearance). Any finding that involves a track or via keeps its
ordinary code and severity.

On a 148-board sample of the public PCBench corpus (KiCad 5), 145 boards
import; the three failures use surface pads on both outer layers.

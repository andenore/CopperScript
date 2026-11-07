# Panelization plan

Status (7 November 2026): steps 1-6 implemented, and fab presets with
checks (the first step of [automatic layout and tabs](#automatic-layout-and-tabs))
(usage: [panelization](panelization.md)). CopperAntennaLab's 18 coupons
panelize into one 222 x 184 mm board that refills and passes native DRC
with only the five radiator tips excused. Not yet done from the steps below: placing tabs on named
edges, and the source digest in `route-board`'s PCB (the routed outline is
checked against the source instead). First consumer:
CopperAntennaLab's 18 antenna coupons, which must reach a fab as one panel
with mouse-bite tabs kept off radiators and connector launches.

## Goal

A `panel` source document that places already-routed boards on a fab panel,
and `copper panelize`, which writes that panel as one `.kicad_pcb` (with its
`.kicad_pro` and `.kicad_dru`) so the existing native tools run on it
unchanged: `copper fill`, `copper drc`, `copper plot` and
`copper export-manufacturing --drc native --plotter native`.

What CopperScript adds over a generic panelizer is board intent: it knows
each board's named edges, antenna regions opened to the perimeter and body
overhangs, so it can refuse a tab on a radiator edge or a connector launch,
and refuse a neighbour or rail inside a connector body's envelope.

Non-goals for the first version: V-scoring, automatic tab placement,
panelizing unrouted boards, mixing stack-ups or design rules, and assembly
panel data (paste stencil frames, panel-level BOM merging beyond prefixed
references).

## Source

```copper
panel AntennaPanel {
    spacing = 2 mm;                  // milled gap between boards and rails
    rails = { top = 5 mm; bottom = 5 mm; left = 0 mm; right = 0 mm; };
    tab_width = 3 mm;
    mouse_bite = { drill = 0.5 mm; pitch = 0.75 mm; inset = 0.25 mm; };
    fiducials = 3;                   // on the rails, mask-defined 1 mm copper dots
    tooling_hole = 1.152 mm;         // non-plated, one per rail corner

    board A1 {
        source = "coupons/a1_ifa_long.copper";
        pcb = "build/a1_ifa_long/a1_ifa_long.kicad_pcb";
        at = (5 mm, 7 mm);
        rotation = 0 deg;
    }
    tab a1_left { board = A1; side = left; at = 30 mm; }   // board-local offset along that side
    tab a1_right { board = A1; side = right; at = 30 mm; width = 4 mm; }
}
```

- A new `DocumentKind::Panel`, parsed with the macro grammar's generic root
  properties and `KIND NAME { ... }` blocks (no new syntax); lowering
  validates kinds, required properties, units, unique names and references.
- `side` names a side of a rectangular board outline (`left`, `right`, `top`,
  `bottom`) in the board's own frame; a named mechanical `edge` of the board
  source is accepted too. `at` is measured along that side from its start.
- Every tab bridges the gap to whatever lies across it: a rail or a
  neighbouring board. A tab with nothing across it is an error.

## Pipeline

1. **Resolve** each board: compile and physicalize its `source` (as
   `synthesize-antenna` does) for its outline, named edges, antenna edge
   openings and body overhangs; read its routed `pcb`. Check that the
   PCB's outline equals the source's, and that it was exported from that
   source (step 4 adds the source digest to `route-board`'s PCB; a PCB
   without one is refused unless `--allow-unverified-boards`).
2. **Compose** at the S-expression level: transform every placed item
   (footprint `at`, track and arc points, vias, zone outlines, graphics,
   texts) by the board's pose; prefix every net (`A1/GND`) and footprint
   reference (`A1-J1`) with the instance name so boards stay electrically
   separate and references unique; drop zone fills (the panel is refilled);
   drop each board's `Edge.Cuts`. Stack-ups, layer sets and design rules
   must be identical across boards (refused otherwise) and are copied.
3. **Frame**: the panel outline is the union of the board outlines, the
   tabs and the rails; `Edge.Cuts` is its boundary (outer and inner
   contours). Mouse bites are non-plated holes along each tab's board edge,
   `inset` into the board; fiducials and tooling holes go on the rails as
   footprints excluded from the BOM.
4. **Check** before writing:
   - tabs on an antenna region's opened perimeter (plus its keepout margin)
     or on a body-overhang interval fail, naming the board, side and
     interval;
   - a body overhang's envelope (the declared allowance beyond the edge)
     must not reach a rail or another board, unless the panel declares
     `assemble_after_depanel = [J1, ...]` for that board's components;
   - tabs must not overlap each other or a board corner radius, and mouse
     bites keep the board's copper-to-edge clearance (left to native DRC on
     the composed board as well).
5. **Write** `panel.kicad_pcb`, `.kicad_pro`, `.kicad_dru` and a
   `copperscript-panel/v0.1` report: boards and poses, tabs and the gap each
   bridges, mouse-bite counts, refused intervals and the checks run. Then the
   native fill, DRC and plot run on the result.

## Steps

Each step is one reviewable change with tests.

1. **Language**: `panel` documents, lowering and validation; parser and
   lowering unit tests; a language-reference section.
2. **Composition** (new crate `copper-panel`): S-expression tree with a
   printer that preserves atoms, pose transforms of every item kind,
   net/reference prefixing, rule and stack-up equality; tested on two
   synthetic routed boards (a composed board re-parses with `copper-cam`,
   copper and drill counts are preserved, nets stay separate).
3. **Frame**: outline union, tabs, mouse bites, rails, fiducials and tooling
   holes; tests for gaps, tab bridging and hole placement.
4. **Board intent**: the source digest in `route-board`'s PCB; forbidden
   tab intervals from antenna openings and overhangs; the overhang
   envelope check; tests with a synthesized antenna board and an SMA
   overhang.
5. **CLI and report**: `copper panelize PANEL -o OUT [--locked --offline]`,
   the report, `copper issues` causes, an `examples/panel` with two small
   boards, docs (`docs/panelization.md`, the docs index), and an end-to-end
   test through `copper fill` and `copper drc`.
6. **CopperAntennaLab** (done): the coupon panel(s), tabs on ground-side edges,
   native fill, DRC and plot, and a CAM comparison against KiCad's plot.

## Automatic layout and tabs

Every board position and tab is written by hand so far. The next steps,
from a survey of KiKit, CAM panel optimizers, fab rules and the packing
literature (2026-11-07), each one change with tests:

1. **Presets and fab checks** (done): `preset`, `fiducial_offset`,
   `--dump-config`, `--strict`, the report's `config` and `checks`.
2. **Automatic tabs** on hand layouts: for each board side, the stretches
   that face a rail or another board's real outline across the gap, minus
   the forbidden intervals (antenna regions plus margin, overhangs, a
   `no_tab` list, copper near the edge, stress-sensitive parts); tabs spread
   evenly by the preset's spacing, at least a minimum per board, and every
   side that cannot take one reported, never dropped silently.
3. **Automatic layout**: each board as an envelope (outline, overhangs not
   assembled after depanelization, half the gap) with the rotations its
   intent allows; columns or rows of mixed heights first (longest first into
   the shortest column), MaxRects for mixed sizes; candidate panel sizes
   scored with their tabs placed, by a fixed order of integer criteria and a
   total tie-break, so the output is the same on every run and platform.
4. **Design limits and families**: split boards over panels within the
   preset's design limit, keeping declared families together; a lock file
   freezes a chosen layout.
5. **Irregular outlines**: rectilinear envelopes and notches.
6. **V-score** on guillotine cut lines that pass the preset's rules.

## Questions for the first consumer

- The edge-launch SMAs overhang the bottom edges by about 9.5 mm: either
  those edges face the panel's outside (no rail there) or the jacks are
  soldered after depanelization (`assemble_after_depanel`). Hand assembly
  makes the second easy.
- JLC panel limits (size, rail and tab conventions, whether mouse bites or
  V-cuts are preferred) should be confirmed against the order form.

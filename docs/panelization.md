# Panelization

A `panel` document places already-routed boards on a fabrication panel;
`copper panelize` writes the panel as one `.kicad_pcb` with its `.kicad_pro`
and `.kicad_dru`, so the native tools run on it unchanged. Design and steps:
[panelization plan](panelization-plan.md); runnable example:
[examples/panel](../examples/panel/README.md).

```copper
panel ExamplePanel {
    spacing = 2 mm;
    rails = { top = 5 mm; bottom = 5 mm; };
    tab_width = 3 mm;
    mouse_bite = { drill = 0.5 mm; pitch = 0.75 mm; inset = 0.25 mm; };
    fiducials = 3;
    tooling_hole = 1.152 mm;

    board MONO { source = "../antenna_synthesis/board.copper"; pcb = "../../build/panel-example/mono/mono.kicad_pcb"; at = (0 mm, 0 mm); }
    board IFA { source = "../antenna_synthesis/inverted_f.copper"; pcb = "../../build/panel-example/ifa/ifa.kicad_pcb"; at = (42 mm, 0 mm); }

    // The monopole's region spans x 18-40 on its top side and y 0-6 on its right.
    tab mono_top { board = MONO; side = top; at = 8 mm; }
    tab mono_bottom { board = MONO; side = bottom; at = 20 mm; }
    tab mono_ifa { board = MONO; side = right; at = 18 mm; }
    // The inverted-F's region starts at x 12 on its top side.
    tab ifa_top { board = IFA; side = top; at = 5 mm; }
    tab ifa_bottom { board = IFA; side = bottom; at = 20 mm; }
}
```

- `preset = "NAME"` takes every property the panel leaves out from a fab
  preset and checks the composed panel against that fab's rules (see
  [Presets and fab checks](#presets-and-fab-checks)).
- `fiducial_offset` puts the fiducial centres that far from the panel's
  outer edge (default: the middle of their rail); the 2 mm mask opening must
  stay on the rail.
- `cutter` is the router bit's diameter (default 2 mm; the presets give
  their own). Every inside corner of the milled outline and every slot end
  comes out rounded to its radius, as the router cuts them, and a cutter
  wider than `spacing` is an error: it could not mill the gaps.
- `frame_fill` (default `true`) keeps the area farther than `spacing` from
  every board, and from every body fitted with the panel, as solid frame
  joined to the rails: only a `spacing` channel is milled round each board,
  and a tab bridges that channel to the frame. `frame_fill = false` cuts that
  area away as before; each piece of it drops out of the router as a loose
  slug, and the report lists it under `milling.pockets`.
- `source` is the board's `.copper` and `pcb` its routed `.kicad_pcb` (the
  `.kicad_pro` and `.kicad_dru` beside it are read too), relative to the
  panel file.
- `assemble_after_depanel = ["BOARD.REFERENCE", ...]` lists component bodies
  fitted after depanelization: they may overhang a rail or a neighbour, and
  the panel marks them out of its position files and BOM.
- Board names use letters, digits and underscores, and differ ignoring
  case: the panel prefixes their nets (`NAME/`) and references (`NAME-`).
  `rail`, `frame` and `tab` are reserved (the report names a tab's far end
  `rail` or `frame`; the frame's mouse bites are footprints `TAB-NAME`).
- `at` puts the corner of the board's outline bounding box; `rotation` is a
  multiple of 90 deg, counter-clockwise as seen in KiCad.
- A tab names its board, a `side` of the board's rectangular outline in the
  board's own frame (`left`, `right`, `top`, `bottom`), the distance `at` of
  its centre along that side (from the left or top end), and optionally its
  `width`. It bridges the gap to the nearest frame (rail or frame fill) or
  board across it; a tab with nothing across within 20 mm is an error.

## What panelize does

1. Compiles each board's source for its edge intent and reads its routed
   board. The routed board must come from that source: the outlines and
   the footprint references must agree (with the `CS_MH_`/`CS_MS_`
   footprints the export adds for mechanical holes and slots), and every
   synthesized radiator must lie on the routed board's copper (feed net and
   layer). A source whose antenna `tuned_for` is stale (an input of the
   full-wave model changed, or a new model revision) is refused, as
   `route-board` refuses it: re-run `copper synthesize-antenna` first. What the routed board shows itself counts too, whatever the
   source says: its rule areas that forbid copper pours, where they cover a
   side (ending on the edge or crossing it: antenna regions, connector
   launches; curved outlines followed to within a quarter micron, contours
   inside others taken as holes), stay
   clear of tabs, plus 1 mm; and each stretch where a footprint courtyard
   reaches beyond a side is a body overhang over exactly that stretch (its
   lines, circles and arcs taken exactly, however the footprint is turned;
   two separate crossings, of one arc included, leave the stretch between
   them to tabs).
2. Moves every item by the board's pose, prefixes nets (`MONO/GND`; nets
   given by name, as KiCad 10 writes them, stay named) and
   references (`MONO-J1`) so boards stay electrically separate, derives
   fresh UUIDs, drops zone fills and the boards' outlines, and clips a zone
   or rule area that reaches past its board to the board's outline (alone,
   the board edge stopped it; a zone cut into pieces becomes one zone per
   piece). Stack-ups, layer
   sets, project rules and the unconditional custom rules (in the same
   order) must agree; those rules are kept once, first, and are the only
   ones between two boards' items, whatever order the boards are declared
   in. A board with conditional rules repeats its whole rule list in its own
   order (the last matching rule wins), with the names its conditions give
   prefixed (`Reference` and `NetName` comparisons and the footprint and
   diff-pair functions such as `memberOfFootprint('J1')`, in either quote) (an empty name, and one of only `*`, stays as written) and each
   rule limited to pairs where
   neither item belongs to another board and at least one belongs to it: on
   the board, against the frame, the board edge or an unnetted hole, its
   rules apply as on the board alone.
   Copper on no net (tracks, vias, zones, copper graphics and copper text)
   on any board is refused when the panel has conditional rules, since that
   copper carries neither prefix.
3. Builds the frame: rails and the frame fill, tabs, mouse-bite holes
   `inset` into every board a tab touches (both ends when it joins two
   boards), fiducials and tooling holes, and the outline on `Edge.Cuts`:
   what remains of the panel rectangle once a round `cutter` has milled the
   space between boards, tabs and frame. The cutter cannot reach into the
   corners where a tab meets a board or the frame, so it leaves a fillet of
   its radius in each; a tab's row of mouse bites, centred across it, goes
   on at its pitch up to a cutter radius past both tab ends to break through
   those fillets, where the extra holes stay on the board and off the
   stretches kept clear of tabs (elsewhere the fillet stays as a small nub).
   Where a tab joins a board its edge leaves `Edge.Cuts`, and after breaking
   the mouse-bite walls are the board edge, so a keep-out inside the board
   along the landing holds pours, tracks, vias and pads back as the edge
   did: as deep as the bites cut (`inset` plus half the drill) plus the
   largest copper-to-edge clearance the rules ask for, and that clearance
   plus the cutter's radius past both tab ends, clipped to the board. The mouse-bite holes are cut
   out of it (10 um wider than drilled): they are the panel's own, and
   KiCad would otherwise report each as an item the keep-out forbids.
4. Refuses, naming the board, side and interval:
   - a tab on an antenna region opened to the perimeter, or within 1 mm of
     one (round a corner onto the next side too);
   - a tab on a body overhang;
   - an overhanging body that reaches a rail or another board, unless
     `assemble_after_depanel` lists `BOARD.REFERENCE`: a routed courtyard
     by its real shape (past a board corner included, and over a tab's
     mouse bites inside the board too), a body overhang the source declares
     by its stretch and distance;
   - a tab whose far end does not land with its whole width on the board
     or rail it bridges to;
   - boards closer than `spacing`, measured edge to edge in every
     direction (diagonal neighbours included), tabs off their side, and
     tabs that overlap anywhere on the panel (the same bridge declared from
     both boards included);
   - a tab closer than `spacing` to a board it passes (only its own board
     and what it bridges to may touch it), or to another tab, touching
     included (the router cannot mill a narrower slot, and touching tabs
     crowd their mouse bites), and a tab no wider than its mouse-bite drill;
   - fiducials and tooling holes that would leave the panel or overlap
     (a panel too narrow for them), and tooling holes without 0.2 mm of
     rail on either side;
   - two boards' courtyards (or declared bodies) on the same side that
     meet between the boards, unless one is fitted after depanelization;
   - a panel that falls apart: boards, tabs and frame must be one piece;
   - mouse bites of two tabs closer than their pitch (each row runs a cutter
     radius past its tab), and two boards the cutter cannot mill apart.
5. Writes `copperscript-panel/v0.1` beside the board (`OUT.panel.json`, or
   `--report`): boards, tabs (what each bridges, its gap and hole count),
   keep-clear intervals, overhangs and the radiators' open ends, the
   effective configuration with each value's origin (`config`), the
   preset's fab checks (`checks`) and `milling`: the cutter, whether the
   frame is filled, the area milled away (`cut_area_mm2`) and the pockets cut
   out as slugs.

## Workflow

```sh
copper panelize panel.copper -o build/panel/panel.kicad_pcb --locked --offline
copper fill build/panel/panel.kicad_pcb -o build/panel/filled.kicad_pcb
copper drc build/panel/filled.kicad_pcb --project build/panel/panel.kicad_pro --rules build/panel/panel.kicad_dru --expected-open-ends build/panel/panel.panel.json
copper plot -o build/panel/plot build/panel/filled.kicad_pcb
```

`--expected-open-ends` excuses only the dangling track ends the report
lists (synthesized radiators' open tips), at that point, on that net and
layer, as `route-board`'s native verification does for one board.

## Presets and fab checks

`preset = "jlcpcb";` fills every property the panel does not set, then
`copper panelize` checks the composed panel against the rules the fab
documents for customer panels. Each result has the value found, the limit,
a note and the page it comes from; values were read on 2026-11-07.

| Preset | Defaults | Rules |
|---|---|---|
| `jlcpcb` | 2 mm gap, 5 mm top and bottom rails, 5 mm tabs, 0.6 mm mouse bites at 0.95 mm pitch, 3 fiducials 3.85 mm from the edge, 2 mm tooling holes | gap >= 1.6 mm, rails >= 3 mm, tabs >= 5 mm, drill 0.5-0.8 mm, web >= 0.3 mm, >= 5 holes per tab (warn), 2 mm tooling (warn), fiducial offset (warn), <= 10 designs, <= 5 free designs (warn) |
| `jlcpcb-smt` | as `jlcpcb` | as `jlcpcb`, plus rails >= 5 mm top and bottom, fiducials and 2 mm tooling holes required, panel 70 x 70 mm minimum and 250 x 250 mm recommended maximum (warn) |
| `pcbway` | 1.6 mm gap, 5 mm top and bottom rails, 5 mm tabs, 0.6 mm mouse bites at 0.95 mm pitch, 3 fiducials, 2 mm tooling holes | gap >= 1.6 mm, rails >= 3 mm, tabs >= 2 mm, drill >= 0.45 mm, web >= 0.35 mm, >= 5 holes per tab, 2 mm tooling, assembly panel 50 x 50 to 330 x 530 mm (warns) |
| `eurocircuits` | 2 mm gap, 5 mm rails on four sides, no fiducials or tooling (Eurocircuits adds them and places the tabs) | gap >= 2 mm, 7 mm from every board to the panel edge, border on four sides >= 5 mm and assembly panel 36 x 46 to 326 x 426 mm (warns) |

A `fail` is outside a documented limit, a `warn` a recommendation or a cost.
The `min_spacing` check reads the smallest gap between boards on the panel,
not only the configured `spacing`. Checks never refuse a panel: they are
printed after the summary and kept in
the report's `checks` block; `--strict` makes any `fail` exit 1. Where a
fab's pages disagree, the rule's note says so: JLCPCB's mouse-bite web
(0.3 mm floor in its panel guide, 0.2-0.3 mm on its capabilities page) and
its design fee (past the fifth design in its panelization help, past the
first on its extra-charges page). Identical boards count as one design.

`copper panelize PANEL --dump-config` prints the effective values, each
with its origin (`explicit`, `preset NAME` or `default`), and the preset's
rules with their limits and sources, without composing anything.

## Limits

Rectangular board outlines for side tabs (named edges are parsed but not
placed yet); no cutouts, arcs or circles in board outlines; no zones inside
footprints; no net-class assignments; no V-scoring or automatic tab
placement. A panel is fabrication geometry: it qualifies nothing about the
boards on it. Presets cover the rules the fabs publish; the fab's order
form and engineering review still decide.

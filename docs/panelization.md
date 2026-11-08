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
- `source` is the board's `.copper` and `pcb` its routed `.kicad_pcb` (the
  `.kicad_pro` and `.kicad_dru` beside it are read too), relative to the
  panel file.
- `assemble_after_depanel = ["BOARD.REFERENCE", ...]` lists component bodies
  fitted after depanelization: they may overhang a rail or a neighbour.
- Board names use letters, digits and underscores, and differ ignoring
  case: the panel prefixes their nets (`NAME/`) and references (`NAME-`).
- `at` puts the corner of the board's outline bounding box; `rotation` is a
  multiple of 90 deg, counter-clockwise as seen in KiCad.
- A tab names its board, a `side` of the board's rectangular outline in the
  board's own frame (`left`, `right`, `top`, `bottom`), the distance `at` of
  its centre along that side (from the left or top end), and optionally its
  `width`. It bridges the gap to the nearest rail or board across it; a tab
  with nothing across within 20 mm is an error.

## What panelize does

1. Compiles each board's source for its edge intent and reads its routed
   board. The routed board must come from that source: the outlines and
   the footprint references must agree (with the `CS_MH_`/`CS_MS_`
   footprints the export adds for mechanical holes and slots), and every
   synthesized radiator must lie on the routed board's copper (feed net and
   layer). What the routed board shows itself counts too, whatever the
   source says: its rule areas that forbid copper pours along its sides
   (antenna regions, connector launches) stay clear of tabs, plus 1 mm, and
   every footprint courtyard reaching beyond a side is a body overhang.
2. Moves every item by the board's pose, prefixes nets (`MONO/GND`) and
   references (`MONO-J1`) so boards stay electrically separate, derives
   fresh UUIDs, drops zone fills and the boards' outlines. Stack-ups, layer
   sets, project rules and unconditional custom rules must agree;
   conditional rules are kept per board with their `Reference` and
   `NetName` comparisons rewritten, and limited to that board's items (its
   nets and references), so one board's rule cannot change another's
   clearances. A board with copper on no net and outside footprints is
   refused when it has conditional rules, since that copper carries
   neither prefix.
3. Builds the frame: rails, tabs, mouse-bite holes `inset` into every board
   a tab touches (both ends when it joins two boards), fiducials and tooling
   holes, and the outline (boards, tabs and rails united) on `Edge.Cuts`.
4. Refuses, naming the board, side and interval:
   - a tab on an antenna region opened to the perimeter (plus 1 mm);
   - a tab on a body overhang;
   - an overhanging body that reaches a rail or another board, unless
     `assemble_after_depanel` lists `BOARD.REFERENCE`;
   - boards closer than `spacing`, measured edge to edge in every
     direction (diagonal neighbours included), tabs off their side, and
     tabs that overlap anywhere on the panel (the same bridge declared from
     both boards included);
   - a panel that falls apart: boards, tabs and rails must be one piece.
5. Writes `copperscript-panel/v0.1` beside the board (`OUT.panel.json`, or
   `--report`): boards, tabs (what each bridges, its gap and hole count),
   keep-clear intervals, overhangs and the radiators' open ends, the
   effective configuration with each value's origin (`config`) and the
   preset's fab checks (`checks`).

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

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

- `source` is the board's `.copper` and `pcb` its routed `.kicad_pcb` (the
  `.kicad_pro` and `.kicad_dru` beside it are read too), relative to the
  panel file.
- `assemble_after_depanel = ["BOARD.REFERENCE", ...]` lists component bodies
  fitted after depanelization: they may overhang a rail or a neighbour.
- `at` puts the corner of the board's outline bounding box; `rotation` is a
  multiple of 90 deg, counter-clockwise as seen in KiCad.
- A tab names its board, a `side` of the board's rectangular outline in the
  board's own frame (`left`, `right`, `top`, `bottom`), the distance `at` of
  its centre along that side (from the left or top end), and optionally its
  `width`. It bridges the gap to the nearest rail or board across it; a tab
  with nothing across within 20 mm is an error.

## What panelize does

1. Compiles each board's source for its edge intent and reads its routed
   board. The routed outline must match the source's.
2. Moves every item by the board's pose, prefixes nets (`MONO/GND`) and
   references (`MONO-J1`) so boards stay electrically separate, derives
   fresh UUIDs, drops zone fills and the boards' outlines. Stack-ups, layer
   sets, project rules and unconditional custom rules must agree;
   conditional rules are kept per board with their `Reference` and
   `NetName` comparisons rewritten.
3. Builds the frame: rails, tabs, mouse-bite holes `inset` into every board
   a tab touches (both ends when it joins two boards), fiducials and tooling
   holes, and the outline (boards, tabs and rails united) on `Edge.Cuts`.
4. Refuses, naming the board, side and interval:
   - a tab on an antenna region opened to the perimeter (plus 1 mm);
   - a tab on a body overhang;
   - an overhanging body that reaches a rail or another board, unless
     `assemble_after_depanel` lists `BOARD.REFERENCE`;
   - boards closer than `spacing`, overlapping tabs, tabs off their side.
5. Writes `copperscript-panel/v0.1` beside the board (`OUT.panel.json`, or
   `--report`): boards, tabs (what each bridges, its gap and hole count),
   keep-clear intervals, overhangs and the radiators' open ends.

## Workflow

```sh
copper panelize panel.copper -o build/panel/panel.kicad_pcb --locked --offline
copper fill build/panel/panel.kicad_pcb -o build/panel/filled.kicad_pcb
copper drc build/panel/filled.kicad_pcb --project build/panel/panel.kicad_pro --rules build/panel/panel.kicad_dru --expected-open-ends build/panel/panel.panel.json
copper plot -o build/panel/plot build/panel/filled.kicad_pcb
```

`--expected-open-ends` excuses only the dangling track ends the report
lists (synthesized radiators' open tips), at that point on that net, as
`route-board`'s native verification does for one board.

## Limits

Rectangular board outlines for side tabs (named edges are parsed but not
placed yet); no cutouts, arcs or circles in board outlines; no zones inside
footprints; no net-class assignments; no V-scoring or automatic tab
placement. A panel is fabrication geometry: it qualifies nothing about the
boards on it, and the fab's own panel rules (size, rails, tab and drill
conventions) still apply.

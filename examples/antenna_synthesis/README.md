# Antenna synthesis probe

A synthetic two-component board that reserves a 22 x 6 mm region in its
upper-right corner and declares a 2.4 GHz band for it:

```copper
constraint antenna(ANT1.FEED) {
    name = "bt";
    x = 18mm; y = 0mm; width = 22mm; height = 6mm;
    ground = GND;
    bands = "2400 MHz-2483.5 MHz";
    track_width = 1mm;
}
```

The compiler lowers the declaration to a generated one-member hard macro: a
quarter-wave monopole scaled by the stack-up's effective permittivity, locked
copper on `F.Cu`, a private region on every copper layer that blocks tracks,
vias and fill, a zone keepout opened to the board perimeter, a placement
keepout for other components, and the feed pad as the macro's one external
port. The feed component is fixed just outside the region on the ground side.
See [the language reference](../../docs/language-reference.md#trace-antenna-synthesis)
and [the research note](../../docs/pcb-antenna-synthesis-research.md).

From the repository root:

```sh
copper check examples/antenna_synthesis/board.copper --locked --offline
copper route-board examples/antenna_synthesis/board.copper -o build/antenna-synthesis.kicad_pcb --report build/antenna-synthesis-route.json --locked --offline
```

`too_short.copper` is the same board with an 8 mm region; its synthesis is
refused with the required and available path lengths.

This is a first-order template, not an antenna design. There is no field
solution, the host ground plane and enclosure are not modelled, matching values
are not computed, and nothing here establishes return loss, radiation or
regulatory compliance. Bench tuning in the final enclosure remains mandatory;
the generated macro records these limits in its `unresolved` list.

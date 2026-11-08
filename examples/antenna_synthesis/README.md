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

Without a `geometry` the compiler uses a first-order template: a quarter-wave
monopole scaled by the stack-up's microstrip permittivity. It lowers to a
generated one-member hard macro: locked copper on `F.Cu`, a private region on
every copper layer that blocks tracks, vias and fill, a zone keepout opened to
the board perimeter, a placement keepout for other components, and the feed
pad as the macro's one external port. The feed component is fixed just outside
the region on the ground side.

`copper synthesize-antenna` tunes the antenna in a full-wave model of this
board and prints the `geometry` and `tuned_for` lines to add to the
declaration. Expect minutes per coarse evaluation and longer for the fine
verification; `--budget` caps the search.

From the repository root:

```sh
copper check examples/antenna_synthesis/board.copper --locked --offline
copper synthesize-antenna examples/antenna_synthesis/board.copper --report build/antenna-synthesis.json --locked --offline
copper route-board examples/antenna_synthesis/board.copper -o build/antenna-synthesis.kicad_pcb --report build/antenna-synthesis-route.json --locked --offline
```

On this board the template lowers to `monopole height=3mm length=17.46mm`.
Verified on the fine mesh (`--verify` on the board without a `geometry`), it
resonates at 2.73 GHz, 12 % above the band centre, and misses -10 dB even
with its best L-section match (-8.8 dB worst in band, 69 % radiation
efficiency), because its permittivity assumes ground under the trace.
Synthesis starts from the same quarter-wave length at the region's full
height (5.3 mm), which resonates at 2.93 GHz on the coarse mesh, and tunes it
from there.
`board.copper` carries the synthesized geometry and its `tuned_for` digest: a
22.73 mm monopole at 2.38 GHz that meets -10 dB over the band with 0.88 nH
series then 2.57 pF shunt from the antenna side, at 86 % radiation efficiency
on the fine mesh (model revision 4; the synthesis takes about three and a
half minutes on a 6-core laptop). Changing an input of the model (outline, stack-up, ground
zone, region, track rules, feed or band) makes lowering fail until the
synthesis is re-run.
See [the language reference](../../docs/language-reference.md#trace-antenna-synthesis)
and [the research note](../../docs/pcb-antenna-synthesis-research.md).

`too_short.copper` is the same board with an 8 mm region; its template is
refused with the length it needs and a pointer to meanders.
`inverted_f.copper` adds the two-land link `R1` between the feed net and
ground and declares `topology = inverted_f; short = R1;`: the macro places
the link as its second member, runs the short leg to its feed land and a
ground stub and via from its ground land.

Synthesis is simulation evidence for one model of the board: components,
other nets, the enclosure and cables are not in it, ground layers are assumed
stitched, and matching parts are ideal. Bench tuning in the final enclosure
remains mandatory; the generated macro records these limits in its
`unresolved` list.

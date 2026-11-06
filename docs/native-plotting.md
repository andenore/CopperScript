# Native plotting without KiCad

`copper plot` writes the fabrication files of a board without running KiCad:
one Gerber X2 file per layer, plated and non-plated Excellon drills, an
IPC-D-356A netlist and a component position CSV. `copper render` draws the
plotted layers as an SVG illustration. `copper compare-cam` checks
two such file sets against each other, typically KiCad's plot and the native
plot of the same board.

```sh
copper plot build/board.kicad_pcb -o build/native-plot
copper compare-cam build/kicad-plot build/native-plot --report build/cam-compare.json
```

`plot` reads the `filled_polygon`s saved in the board and never fills zones
itself. They come from a KiCad refill (`kicad-cli pcb drc --refill-zones
--save-board`) or from [`copper fill`](native-filling.md). It runs no DRC. On
a board that was never filled, every zone plots empty, and the summary line
reports the number of fill areas read.

`copper export-manufacturing --plotter native` uses this plotter in the release
path: the board is refilled and checked in the staging directory (by KiCad, or
natively with `--drc native`), then plotted natively.

## What is plotted

The content follows KiCad's own plot of the same board with the options
`export-manufacturing` passes (`--subtract-soldermask`, absolute drill origin,
separate PTH/NPTH files, millimetres):

- Copper layers: zone fills as regions, tracks and arcs with round pens, vias,
  and pads on that layer. NPTH pads no larger than their hole carry no copper.
  Copper graphics and text are included. Pads carry `TO.P`/`TO.C` attributes,
  and copper objects carry their `TO.N` net.
- Masks (negative polarity): pad openings grown by the pad, footprint or board
  mask margin, untented vias, and graphics drawn on the mask layer.
- Paste: pads with paste on that side, grown by the paste margin and ratio.
- Silkscreen: graphics and stroke text, then the mask openings of that side in
  clear polarity.
- Edge.Cuts: the outline strokes, with the `Profile` aperture function.
- Drills: vias and plated holes in `-PTH.drl`, non-plated holes in
  `-NPTH.drl`, and oval holes as `G85` slots. Tools are sorted by diameter.
- IPC-D-356A: one record per via and per copper pad, in imperial units.
  Net names longer than 14 columns get standard `NNAME` aliases.
- Positions: KiCad's `Ref,Val,Package,PosX,PosY,Rot,Side` columns, so
  `write_jlcpcb_cpl` can consume them unchanged.

Coordinates are integer nanometres (Gerber format 4.6 mm), with Y negated as
KiCad does. Pad angles not on a quarter turn are rounded to the nanometre. No
creation date is written, so the same board always gives the same bytes.

Text uses the newstroke font with KiCad's own layout constants, so silkscreen
matches KiCad's to within a pixel. The licence of the vendored glyph data is
still to be confirmed: see `crates/copper-cam/fonts/README.md`.

### Fails closed

The reader refuses a board when it cannot reproduce the plotted geometry,
rather than plotting an approximation. Refused constructs:

- custom, trapezoid or chamfered pads, pad stacks and drill offsets
- `remove_unused_layers yes` on pads or vias
- blind, buried and micro vias
- hatched zone fills, dashed strokes and hatched graphic fills
- knockout text, TrueType text, text variables and markup, tabs
- visible footprint text on a plotted layer
- dimensions, images, tables, text boxes and Bézier curves on a plotted layer
- a nonzero `solder_mask_min_width`, which merges mask openings

## Comparison

`compare-cam` loads every Gerber, drill, `.d356` and position CSV under each
directory and pairs the Gerbers by their `TF.FileFunction`. Each pair is
rasterised on one shared grid (default 10 µm). The comparison reports:

- the raw XOR area;
- the difference that survives one erosion pass (`--erosion`). Erosion removes
  the single-pixel edge noise of rasterising the same edge from differently
  flattened arcs or rounded coordinates. Surviving regions are located in
  board millimetres;
- drill hits matched within 2 µm (`--hole-tolerance-um`), with slot ends in
  either order;
- IPC-D-356 netlists by connectivity. The same test points must exist on both
  sides and group into the same nets. Renamed nets are listed but do not fail
  the comparison;
- positions per reference, to 0.001 mm and degrees.

Exit status 0 means everything matches, 1 means differences were found and 2
means an error.

The raw difference is the sensitive signal. In mutation tests at the default
pitch, a single removed pad flash and a via hole moved by 10 µm were reported. A track widened by 25 µm per side
gives a significant difference. Widened by 15 µm per side, it survives
erosion only in small spots but shows a large raw difference. Use a finer
`--pitch-um` for tighter checks; memory grows with the square of the pitch.
Geometry inside a same-net fill is invisible in the image by construction.

## Illustrations

`copper render` draws a plot directory as one SVG, without KiCad. Each
`--layer` is a coloured plate drawn in order, bottom first, from the plotted
artwork read back with the comparison reader: dark objects add, clear objects
remove, in file order, so polarity and the soldermask subtraction in the
legend come out as fabricated. `LAYER&WITHIN` draws a layer only where
another is dark: `F.Cu&F.Mask` is the top copper inside the solder-mask
openings, i.e. the exposed copper. Layers are named as in KiCad from each
file's `TF.FileFunction`; colours are `#rgb`/`#rrggbb` with an optional
opacity, and KiCad's default colour is used when none is given.

```sh
copper render build/native-plot -o build/top.svg \
  --layer F.Cu --layer 'F.Cu&F.Mask:#d9a441' --layer 'F.Silkscreen:#3a3f3b' --layer Edge.Cuts
```

The picture is framed on the board profile (`Edge.Cuts`) plus `--margin`
(1 mm by default), in millimetres, viewed from above and never mirrored.
`--background` paints under the profile. To show copper without zone fills,
plot the board before it is filled: the plotter draws no fill it does not
read. KiCad's Gerbers render the same way, so the two plots can be compared
by eye as well as with `compare-cam`.

## Verification (KiCad 10.0.6)

The native plot was compared with `kicad-cli` exports of four refilled boards:
BysseControl-32 (4 layers), CopperBench, CopperAssetTracker and CopperProbe
(6 layers). Results:

- Every copper, mask, paste and profile layer is pixel-identical: the raw
  difference is 0.
- Silkscreen with text differs by at most 0.0116 mm² of single-pixel edges,
  with no significant region.
- All drill hits (446, 1095, 272 and 356) and all positions match.
- Netlist connectivity matches, with 1079, 2317, 826 and 969 records.

The CopperScript-written BysseControl board, unfilled and in KiCad 8 format,
also matches KiCad's plot of the same file.

The netlist comparison uncovered a difference in KiCad: 10.0.6 truncates net
names longer than 14 characters to their last 14 (`S3_RESET_ASSERT` becomes
`3_RESET_ASSERT`) instead of aliasing them. Two nets sharing those 14
characters would then merge in KiCad's netlist. The native netlist keeps
full names through `NNAME` aliases. The installed binary also marks only vias
as `M` midpoints, not unnumbered pads, and the native writer does the same.

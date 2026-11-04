# Native zone filling

`copper fill` fills a board's copper zones without KiCad and writes the board
with the new `filled_polygon`s. All other text of the file is kept byte for
byte. It reads the design rules from the same-stem `.kicad_pro` and
`.kicad_dru`, or from `--project` and `--rules`.

```sh
copper fill build/board.kicad_pcb -o build/board.filled.kicad_pcb
copper plot build/board.filled.kicad_pcb -o build/native-plot
```

Together with [native plotting](native-plotting.md), this gives fabrication
files without KiCad installed. `fill` itself runs no DRC and no CAM check;
[`copper drc`](native-drc.md) does. `export-manufacturing --drc native` fills
with it and gates on the native DRC; KiCad's refill remains the default.

## Algorithm

The filler follows KiCad 10's own (iterative) refill:

1. Start from the zone outline, clipped to the board interior (Edge.Cuts
   loops, cutouts as holes).
2. Cut thermal relief gaps around pads connected by thermals.
3. Knock out every other-net pad, plated hole, via, track, copper graphic and
   copper text. Each grows by its clearance, plus KiCad's 0.5 µm fill margin.
   Also knock out the board edge (edge clearance from the edge line, ignoring
   its width), NPTH holes (hole clearance) and keepouts that forbid pours.
4. Add the four thermal spokes per thermal pad, KiCad's geometry included
   (45° for round pads, else 90°, unless the pad says otherwise), and keep
   those that reach copper.
5. Drop necks narrower than the zone's minimum width: shrink by half of it
   (bevelled), drop fragments smaller than the width, and regrow rounded.
6. Knock out the fills of higher-priority zones on other nets, grown by
   their clearance, and prune minimum width again. Higher-priority same-net
   zones keep their own area.
7. Remove islands. An island is a fill outline whose connected copper (fills,
   tracks, vias, pads of the net) reaches no pad. Each zone's mode applies:
   always, never, or below a minimum area. A zone isolated on every layer is
   left as it is. Refill until the fills are stable, so zones take back
   space freed by removed islands.

Zones are processed by descending priority, ties broken by UUID, as KiCad
does. Curves are approximated within `max_error` (5 µm by default), with
vertices outside the true curve, so a knockout is never smaller than its
rule.

A clearance is the largest applicable value of:

- the board minimum clearance;
- either item's local or net-class clearance;
- the last matching `clearance` custom rule.

Hole and edge clearances work the same way. Custom-rule conditions are
understood for the subset CopperScript writes: `A.`/`B.` `NetName`,
`NetClass` and `Reference`, compared with `==` or `!=`, combined with `&&`,
`||` and parentheses.

### Refused

The filler refuses settings whose effect it does not reproduce, rather than
approximating them:

- hatched fills, corner smoothing, teardrops, `filled_areas_thickness yes`
- copper zones inside footprints
- custom rules with physical clearance, thermal relief, zone connection or
  `disallow` constraints, layer-qualified clearance rules, and unknown
  condition syntax
- everything the [plotter refuses](native-plotting.md#fails-closed)

The writer adds `(filled_areas_thickness no)` to each zone it fills. Without
it, KiCad reads the fills of a pre-20250210 file as outlines stroked by the
minimum width, and its DRC then sees the copper 0.125 mm fatter. For the same
reason the plotter refuses such legacy stroked fills.

## Verification (KiCad 10.0.6)

Each board was filled twice from its unfilled CopperScript output: by
`kicad-cli pcb drc --refill-zones` and by `copper fill`. Both were plotted
natively and compared with `compare-cam`. KiCad's DRC was then run on the
native fill without refilling.

| Board | Largest significant difference per copper layer | KiCad DRC on the native fill |
|---|---|---|
| BysseControl-32 | 0.17 mm² | same as KiCad's own fill: no clearance violations, 0 unconnected |
| CopperBench | 0.005 mm² | same as KiCad's own fill |
| CopperAssetTracker | 0.005 mm² | same as KiCad's own fill |
| CopperProbe | 3.2 mm² (one island, below) | same as KiCad's own fill |
| BysseControl-32, all zones thermal | 0.35 mm² across specks of about 0.002 mm² | same as KiCad's own fill (one `starved_thermal` in both) |

The raw difference is 0.1–4 mm² per layer, on layers of up to 22,000 mm² of
copper. It is single-pixel edge noise from the two fillers' different curve
approximations. The native fill takes 0.5–2.3 s per board; KiCad's refill
and DRC took 6.8 s on CopperBench.

Known differences:

- **Point contacts.** On CopperProbe In3.Cu, a 3.2 mm² region touches the
  main pour only at a single point. Natively the two are 0.8 µm apart, so
  the region is an island and is removed. KiCad's polygons overlap there by a
  fraction of a micron, so KiCad keeps it. A point contact does not survive
  etching, so the copper is floating either way.
- **Borderline pockets.** A pocket whose width is within a few micrometres of
  the minimum width (between a via pair and a passing track, for example) can
  survive in one fill and not the other. KiCad also bridges nearby fragments
  during pruning (`connect_nearby_polys`), which is not reproduced.

## Finding: zone connection codes

KiCad numbers pad `zone_connect` as 0 none, 1 thermal, 2 solid, 3
through-hole thermal. CopperScript's footprint importer and KiCad exporter
use 1 none, 2 thermal, 3 solid, 4 through-hole thermal. The shift cancels
when a footprint round-trips into KiCad. But CopperScript's own model reads a
solid pad (2) as thermal, a thermal pad (1) as none, and rejects
`zone_connect 0`. The native filler follows KiCad's numbering.

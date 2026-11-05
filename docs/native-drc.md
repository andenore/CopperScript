# Native design-rule check

`copper drc` checks a filled board against the design rules of its
`.kicad_pro` and `.kicad_dru` without KiCad. With `--cam`, it also reads
back a directory of plotted Gerbers and drills and checks their
connectivity against the board netlist. It exits with 0 when the board is
clean, 1 when there are findings, and 2 on an error.

```sh
copper fill build/board.kicad_pcb -o build/board.filled.kicad_pcb
copper plot build/board.filled.kicad_pcb -o build/native-plot
copper drc build/board.filled.kicad_pcb --report build/drc.json --cam build/native-plot
```

`copper export-manufacturing --drc native` uses both as its release gate in
place of `kicad-cli pcb drc` (see [manufacturing files](manufacturing-files.md)).
KiCad's DRC remains the default and stays available. The native check is
narrower than KiCad's: what it does not check is listed below, in every
report and in the export manifest.

## Design-rule check

Every copper object is modelled exactly, as a spine grown by a radius:

- round and oval pads, vias and tracks as points and segments;
- rectangular and rounded-rectangle pads as polygons;
- zone fills as their saved polygons.

Arcs are flattened to 10 nm, and their radius is grown by the same amount.
Distances are edge to edge. As in KiCad, a shortfall of up to 0.5 µm
(`DRCEpsilon`) passes. Clearances come from the same rule engine as the
[native filler](native-filling.md):

- the board minimum;
- net classes;
- local pad, footprint and zone overrides;
- custom `clearance`, `hole_clearance` and `edge_clearance` rules.

Violation types use KiCad's names. Severities follow the project's
`rule_severities`, with KiCad 10's defaults where it is silent: a type set to
`ignore` is not reported, and `track_not_centered_on_via` is reported only
when the project enables it. Every finding carries the KiCad UUIDs of its
pads, tracks, vias and zones, so `route-board` can bind it to board objects.

| Check | KiCad type |
|---|---|
| Copper of different nets closer than the clearance, or touching (0 mm) | `clearance` |
| Holes closer than the hole clearance to copper of another net | `hole_clearance` |
| Holes closer together than `min_hole_to_hole` | `hole_to_hole` |
| Copper closer than the edge clearance to Edge.Cuts or a non-plated slot, or outside the outline | `copper_edge_clearance` |
| Track width, via diameter, annular width of vias and plated pads, and hole size, against the board minimums | `track_width`, `via_diameter`, `annular_width`, `drill_out_of_range` |
| Tracks, vias, pads or fills inside a keepout that forbids them | `items_not_allowed` |
| A net whose pads, tracks and vias form separate groups | `unconnected_items` |
| Dangling tracks and vias | `track_dangling`, `via_dangling` |
| A track ending inside a via of its net but off its centre, with no track on that layer reaching the centre | `track_not_centered_on_via` |
| A fill outline whose connected copper reaches no pad | `isolated_copper` |
| A thermally connected pad with fewer than two spokes into the fill | `starved_thermal` |

The thermal check follows KiCad. It grows the pad by half its relief gap and
counts crossings with each fill outline, two per spoke. Spokes into an island
that touches only one object count only when there is nothing else, and
connected tracks running from inside the grown pad into the fill count as
spokes.

The connectivity checks follow KiCad's rules:

- Pads a part joins internally (`jumper_pad_groups`, or
  `duplicate_pad_numbers_are_jumpers`) count as connected.
- A track end is connected when copper of its net comes within the track's
  half width.
- A neighbouring track or via that reaches both ends of a short track
  counts only for the nearer end. A pad or a pour that covers both ends
  counts for both.
- A via is dangling when it connects on fewer than two layers.

### Not checked

- Courtyards: overlaps and `courtyard_clearance` rules.
- Silkscreen: clearance to copper and to the board edge, overlaps, and text
  size.
- Solder mask bridges.
- Footprint library and schematic parity.
- Length, skew and differential-pair rules.
- Keepouts that forbid footprints.

Every custom-rule constraint outside the modelled clearances is also listed
in the report. Constraints that would tighten a check made here are refused
rather than ignored, so a board is never checked against a looser rule than
it declares. These are `track_width`, `via_diameter`, `hole_size`,
`annular_width`, `hole_to_hole`, `connection_width`, `min_resolved_spokes`
and `assertion`.

## CAM read-back

The read-back checks the plotted files themselves, independently of the
model that drew them:

1. Every copper Gerber is read back and rasterised at 10 µm, and its
   8-connected copper regions are labelled.
2. Plated drill hits join the regions they pass through on every layer.
3. Every pad with a net and every via is located in the region under it,
   on each of its copper layers. Layers join only through plated holes,
   and pads a part joins internally count as joined.

The read-back then reports:

- a short, when one region holds two nets;
- an open, when one net is spread over several regions;
- a test point with no copper under it.

## Verification (KiCad 10.0.6)

Each board was checked twice, on KiCad's refill and on the native fill, by
`kicad-cli pcb drc --severity-all` and by `copper drc --cam`. Footprint
library warnings are left out of the KiCad counts, since the scratch copies
had no library table.

| Board | KiCad | Native | Read-back |
|---|---|---|---|
| BysseControl-32 | clean | clean | passed |
| CopperBench | 3 `track_dangling` | the same 3 tracks | passed |
| CopperAssetTracker | clean | clean | passed |
| CopperProbe (unfinished) | 10 `track_dangling`, 11 unconnected | the same tracks; 11 unconnected on the same nets | opens on the same 9 nets |
| BysseControl-32, all zones thermal | 1 `starved_thermal` | the same pad, with the same "spoke into an isolated island" cause | passed |

For dangling tracks, KiCad marks the track start and the native report
marks the dangling end, so findings were matched by track.

Clean boards do not exercise the clearance checks. So BysseControl-32 was
damaged five times, with different random seeds: each time, 25 tracks were
widened by 0.05 to 0.2 mm and 10 vias moved by up to 0.15 mm. In all five,
every KiCad finding (62 to 80 per board, `clearance` and `hole_clearance`)
has a native finding of the same type with the same measured gap,
overlapping copper included. The native report has 2 to 15 more. These are
real violations that KiCad does not list, because `kicad-cli` reports only
the first error per track per layer unless "report all track errors" is set.

`export-manufacturing --drc native --plotter native` on BysseControl-32 ran
without KiCad (with `--kicad-cli` pointing at a missing program) in 10 s,
against 27 s for the KiCad path. Compared with the KiCad export:

- mask, paste, silkscreen and profile layers match;
- drills, netlist connectivity and positions match;
- copper differs only by the fill differences listed in
  [native filling](native-filling.md), at most 0.17 mm² per layer.

The check takes 0.5 to 2 s per board, and the read-back 1 to 9 s.

### Plane verification

`route-board --verify-plane-fill native` was compared with the KiCad
verifier on the three ladder boards that verify planes, routed by the same
commit with the same options (`copper bench --only BOARD`, once with
`--kicad-cli` set to KiCad and once to `native`).

| Board | Routed result | Verification steps | Findings per step |
|---|---|---|---|
| CopperLedRing | identical: 51/51, 40 vias, 553.714 mm; 44 s native, 74 s KiCad | 2 and 2 | identical, UUIDs included |
| CopperAssetTracker | identical: 122/124, 275 vias, 1828.32 mm | 1 and 1 | the same 2 opens on NRF_XC2 and the same repair decision |
| CopperProbe (unfinished) | identical: 228/237, 2570 tracks and vias | 5 and 5 | the same 7 violations; 12 opens natively, 11 in KiCad |

The extra CopperProbe open is a fill difference, not a check difference.
The native check finds exactly KiCad's 11 opens on KiCad's own fill of the
same step. The native fill drops a borderline D18 pocket that KiCad keeps
(see [native filling](native-filling.md)).

Opens are reported as KiCad's ratsnest reports them: the minimum spanning
tree over a net's separate groups, each link between the two closest pads,
vias or track ends. The filled-copper repair builds its proposals from
those two items.

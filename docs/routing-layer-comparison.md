# Full-vertical prototype: four versus six routing layers

This is a bounded routing experiment on `examples/full_vertical/board.copper`,
not a fabrication release. Both runs used the same 100 x 80 mm provisional
outline, resolved KiCad/CopperLib footprints, nominal 0.09 mm clearance,
0.20 mm default track width, one placement candidate, one global feedback
iteration, a 1 mm detailed grid, and GND deferred to a later verified fill.

| Run | Four layers | Six layers |
|---|---:|---:|
| Short search (1 pass, 1,000 states) | 30/58 nets | 31/58 nets |
| Stronger search (2 passes, 5,000 states) | 45/58 nets | 45/58 nets |
| Stronger-search vias | 86 | 91 |
| Stronger-search track segments | 520 | 909 |
| KiCad unconnected items after temporary zone refill | 152 | 110 |
| Native drill-spacing findings | 0 | 2 |
| KiCad isolated-fill findings | 0 | 3 |

Both stronger runs left 13 nets open, including GND, which requires actual
zone-fill and stitching verification. Neither run produced a copper-short or
track-clearance finding. Independent KiCad DRC also reported eight footprint
library lookup/mismatch findings on each draft. The six-layer drill violations
are same-net V3V3 through-vias whose holes are only 0.075 and 0.125 mm apart,
below the 0.25 mm drill-spacing rule. They are real release blockers.

The six-layer stronger draft placed track segments only on F.Cu, In1.Cu, and
In2.Cu; **In3.Cu, In4.Cu, and B.Cu were unused**. The current placer/router
therefore did not exploit the added signal layers. Its cost favors short
front-layer routes, and global guides penalize deeper-layer deviations. The
source's provisional In1.Cu GND zone is not yet treated as a reserved return
plane, so even the reported routing benefit is not a return-path signoff.

The next meaningful routing experiment should reserve the GND plane from
ordinary tracks, make layer allocation/guide costs aware of available signal
layers, and reject closely spaced vias *within the same candidate net* before
committing that route. Then rerun matched four/six settings with native and
KiCad DRC plus verified zone fill. Pin-access failures and search-budget
exhaustion remain important even if more layers are available.

The routing command for each stronger run was:

```console
python -m copperscript route-board examples/full_vertical/board.copper \
  --locked --offline --layers L --fab-profile PROFILE \
  --footprint-root PATH_TO_KICAD_FOOTPRINTS \
  --footprint-root PATH_TO_COPPERLIB_FOOTPRINTS \
  --candidates 1 --feedback-iterations 1 --router-iterations 5 \
  --pitch-mm 1 --passes 2 --search-budget 5000 \
  --constrained-pins-first --progressive-guides --defer-zone-nets \
  --report route-Llayer.json -o route-Llayer.kicad_pcb
```

Use `L=4`, `PROFILE=jlcpcb-four-layer`, or `L=6`,
`PROFILE=jlcpcb-six-layer`. The six-layer profile is a provisional clearance
profile, not a controlled-impedance or CAM-qualified fabrication stackup.

# Via policy and per-layer review

## Working list

- [x] Reject complete via annulus contact with any pad by default, including
  same-net pads in ordinary, critical, package escape and soft rip-up searches.
- [x] Remove automatic ground via-in-pad from the complete-board script.
- [x] Add native physical DRC for unintended same-net via/pad contact.
- [x] Replace clear ordinary perpendicular corners with checked 45-degree
  chamfers; preserve branches, pads, vias and locked input copper. Joint USB
  pair refinement retains responsibility for differential-pair shape.
- [x] Add a reproducible read-only KiCad per-layer audit and SVG plots.
- [ ] Finish profiled full-board rerun with current placement and strict policy.
- [ ] Independently refill/DRC and inspect every saved copper layer.
- [ ] Record remaining bends, local density, opens and escape failures without
  relaxing defaults to obtain closure.

Ordinary vias may not touch even the edge of a same-net pad. Native KiCad DRC
normally accepts that electrical connection, so CopperScript also checks it as
`DRC-VIA-PAD-OVERLAP`. Explicit, qualified `filled-capped` via-in-pad remains a
separate opt-in process; the full-board script does not opt in. This is not a
blanket same-net clearance rule: tracks must still join their pads and vias.

Cleanup of ordinary and local surface-closure paths trims only degree-two
perpendicular octilinear corners, trying decreasing
chamfer sizes. Every new diagonal passes exact copper/keepout/board-edge tests.
Contacts in the cut-away area block cleanup; obstacle-constrained corners are
retained. It neither independently alters paired USB geometry nor edits a saved
KiCad board by hand.

Run the audit with KiCad's bundled Python after a routing run (PowerShell):

```powershell
& 'C:\Program Files\KiCad\10.0\bin\python.exe' scripts/review_routing_layers.py `
  build/full-vertical/<run>/board.kicad_pcb `
  --output-dir build/full-vertical/<run>/layers `
  --kicad-cli 'C:\Program Files\KiCad\10.0\bin\kicad-cli.exe'
```

Outputs include one saved-copper SVG per enabled copper layer and
`layer-review.json`: exact native via/pad collision locations, track lengths,
degree-two sharp turns (90 degrees or greater), branches, non-octilinear segments
and coarse 10mm local track-area estimates. Plane fill is not recomputed by the
audit. Refill/DRC separately before interpreting plane connectivity. These style
and density metrics are not electrical, impedance or manufacturing signoff.

## Historical baseline, 2026-10-02

Audited `build/full-vertical/20261002T080102482942Z/board.kicad_pcb`, then
independently refilled a disposable copy. This predates the fixed mechanical
floorplan and strict via policy; it is not a controlled single-change benchmark.
KiCad found zero unconnected items but ten other findings (eight library findings
and two dangling copper findings). It is not production signoff.

The native shape audit found 37 via/pad contacts: 14 on GND and 23 on other nets.
The ordinary router's same-net exemption, not just opt-in GND via-in-pad, mattered.

| Layer | Track length (mm) | Sharp-turn candidates | Free-corner candidates | Highest 10mm track-area estimate |
| --- | ---: | ---: | ---: | ---: |
| F.Cu | 857.618 | 106 | 76 | 16.38% |
| In1.Cu | 0 | 0 | 0 | 0% (GND plane) |
| In2.Cu | 597.025 | 33 | 31 | 12.64% |
| In3.Cu | 129.765 | 8 | 8 | 6.35% |
| In4.Cu | 142.340 | 12 | 12 | 5.38% |
| B.Cu | 181.813 | 13 | 13 | 6.22% |

Candidates include 153 right-angle turns, eight 135-degree direction changes and
eleven overlapping/backtracking endpoint junctions. A degree-two endpoint is not
necessarily a simple bend: fanout/terminal copper may also meet a track interior.
Free-corner counts exclude pad-center and exact via-center junctions, but do not
prove a legal alternative exists. F.Cu also has two oblique GND closure segments.

Visual inspection: F.Cu is dominated by package access and central component
density; In2.Cu carries the most long inner-layer runs and several detours;
In3.Cu/In4.Cu/B.Cu have spare-looking regions but some local doglegs. The saved
board has no plane fill until independently refilled. In1.Cu then shows the GND
plane rather than missing routing. Sparse signal layers are not automatically
free capacity near terminals: through-via collisions also involve outer pads.
The current preference model favors In2.Cu because it borders the declared plane,
and applies complementary soft headings to the inner signal layers. Congestion
balancing must preserve reference-plane constraints rather than simply equalize
track length on all layers.

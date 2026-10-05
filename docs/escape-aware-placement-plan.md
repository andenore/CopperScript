# Escape-aware placement

## Goal and scope

Replace the 32-pad dense/dense spacing shortcut with a directional, physical
estimate that also catches MCU/sensor and package/obstacle channels. Use free
board space without scattering decoupling networks or moving fixed connectors.
This changes generic physical placement only, not electrical IR, source
connectivity, routing clearance, board outlines or production signoff. The
full-vertical power-plane experiment is separate and is not enabled here.

The motivating saved placement has a 1.6 mm copper-envelope gap between its
STM32 and accelerometer. Two default 0.8 mm via banks plus a 0.2 mm transit track
and four 0.09 mm clearances require a conservative 2.16 mm cross-section before
placement margin. This is a planning estimate, not a universal DRC rule:
staggered vias, same-net contacts and actual package geometry can change access.

## Design

1. Build orientation/side-aware access profiles from actual physical pad shapes
   and package/courtyard envelopes. Count connected SMD pads, including power
   and ground. Through-hole pads already have a layer transition. Unconnected
   pads remain obstacles but do not demand an escape. No 32-pad cutoff.
2. Assign demand to the nearest facing package side. Estimate via-bank rows from
   tangential pad span and the stricter of copper-via and drill pitches. Permit
   bounded lateral staggering by one via pitch at each end of that span, capped
   by the package-side envelope; do not assume unlimited spreading. Use
   actual net widths/clearances, via dimensions, allowed layers, dedicated-plane
   policy and zero-via restrictions. Do not divide surface escape demand by the
   number of copper layers; those vias still need physical landing space.
3. Score only overlapping, facing same-side envelopes. Budget the two sides'
   demanded access depth, a configurable transit lane and placement margin.
   A blocker need not itself be a dense IC. Two-terminal passives do not demand
   their own via banks but still obstruct other packages. Close companions are
   exempt within their proximity unit; rigid macro interiors are exempt because
   they have their own fixed geometry. Other constrained IC pairs still report
   an unmet channel if their constraint prevents relief.
4. Prioritize channel deficit ahead of HPWL in local and final candidate scores.
   Preserve hard legality and relative constraints. Spacing is a soft heuristic:
   fixed or crowded designs may retain a deficit, but must expose it as a
   warning with component pair, direction and measured/target gap.
5. Translate whole rigid/proximity units, including decouplers, in bounded
   deficit-derived grid steps into unused material. Recheck complete legality
   and non-increasing coarse congestion before accepting a move. Preserve fixed
   poses and macro rotations. No example references or coordinates in pcbir.
6. Keep joint ordinary/critical/GND package-access preflight before area routing.
   A zero heuristic deficit is not proof that both packages can escape: only
   actual reserved geometry and existing native acceptance gates establish that.

Default margin is 0.5 mm per demanding side and one transit lane. Dimensions,
lane count and bounded spreading passes/movement are planner options, not
global fabrication limits. Rotate/mirror profiles with the actual footprint,
including already supported 45-degree poses. Profiles are cached within a
board-bound model; new board/rule snapshots get new models. Keep only the latest
XY pose/pair result in incremental caches so placement trials do not accumulate
an unbounded position history.

## Implementation checklist

- [x] Implement the board-bound directional escape model and deficit reports.
- [x] Integrate escape-first scoring into analytical/local/final placement.
- [x] Replace dense-only spreading with bounded constraint-unit separation.
- [x] Expose structured deficit metrics and actionable layout warnings.
- [x] Test mixed package sizes, demand, layer rules, rotations, locks, macros,
  companions, congested boards, determinism and report serialization.
- [x] Compare the saved STM32/sensor poses against new full-vertical placement
  using the same footprints, six-layer stackup and fixed source constraints.
- [x] Verify existing placement and routing/preflight integration; commit the
  verified implementation. A full detailed reroute is a separate closure run.

## Basis and limits

The floorplanning-before-escape principle is consistent with
[TI's PCB escape-routing guide](https://www.ti.com/lit/an/sprad13a/sprad13a.pdf).
Its BGA-specific patterns are not copied into LQFP/LGA algorithms. Existing
clearance, mechanical and package-access checks remain authoritative. The
axis-projected model is deliberately conservative at diagonal/corner channels;
it does not solve exact pin-access allocation, SI, impedance or power integrity.

## Full-vertical placement experiment (2026-10-06)

Used the unchanged source, pinned CopperLib inputs, KiCad 10.0 footprints,
six-layer JLCPCB profile and placement-template scene. Outputs and cProfile
data are ignored under `build/escape-aware-placement-20261006/`.

The saved STM32 at (55, 40) mm and accelerometer at (54, 49) mm had a 1.60 mm
copper-envelope gap and a 1.05 mm courtyard-envelope gap. The new planner chose
STM32 (50, 33) mm / 0 degrees and accelerometer (50, 52) mm / 0 degrees:
11.60 mm copper gap, 11.05 mm courtyard gap. At the new orientation this channel
needs an estimated 4.05 mm courtyard-envelope gap (two MCU via rows, one sensor
row, transit lane and margin). This is a minimum target, not a request to spend
11.6 mm on every pair; other placement objectives and obstacles also affect the
selected poses. No component-specific move or source-coordinate edit was used.

Complete placement legality, relative constraints, all eight fixed-pose rules
and rigid-template geometry passed assertions. Total modeled corridor deficit
dropped from 37.71 mm over all pairs to zero; coarse overflow is zero and HPWL
is 1900.7 mm. The layout report still warns about estimated crossings and lossy
footprint imports, and keeps routing not run / production verification blocked.
No power pour, via-in-pad permission or fabrication clearance changed.

A second profiled run using the bounded pair cache produced byte-identical PCB
and JSON files. Its instrumented placement phase took 185.9 s, with 14.0 s
cumulative in channel evaluation. These are diagnostic timings, not a speedup
benchmark: tests ran concurrently and compilation includes dependency I/O.
Regression tests also compare cached results against fresh models after XY,
rotation and side changes, including reverting poses.

Detailed rerouting, actual joint package-access allocation at these poses and
native routed/fill signoff remain the next closure run. A larger gap and zero
estimated deficit do not prove all exits or board nets are connected.

Validation: the combined placement/API/editor/CLI/package-access suite passed
270 tests. The complete new escape-model file was then rerun with two additional
tight-region/congestion rollback cases (21 passed; 272 unique tests in the
combined placement inventory). The separate global/critical feedback, fanout
and owned-track cleanup suite passed 81 tests. Python compilation and
`git diff --check` passed. One prior fanout test required unused owned copper to
remain; its failure reproduced using HEAD's placer. Updated it to require the
connected, DRC-clean route and removal of the unused tail, without changing
router behavior in this feature.

To repeat placement only in PowerShell (cached dependencies required for
`--offline`; the routing workflow can populate them first):

```powershell
New-Item -ItemType Directory -Force build/escape-aware-placement | Out-Null
uv run python -m pcbir.profiling --output build/escape-aware-placement/placement.prof --module copperscript -- `
  plan-layout examples/full_vertical/board.copper --locked --offline `
  --footprint-root "C:/Program Files/KiCad/10.0/share/kicad/footprints" `
  --layers 6 --fab-profile jlcpcb-six-layer `
  --placement-templates examples/full_vertical/placement_templates.json `
  --escape-margin-mm 0.5 --escape-transit-lanes 1 --candidates 1 `
  --report build/escape-aware-placement/layout-report.json `
  -o build/escape-aware-placement/placed.kicad_pcb
```

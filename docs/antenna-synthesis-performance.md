# Antenna synthesis performance: profile and plan

Measured 7 November 2026 on branch `antenna-speed` (from `origin/main`
8e8f682), `ems` 0.0.5, on a Ryzen 5 7535HS laptop (6 cores / 12 threads,
16 MB L3, 2 x 8 GB DDR5 running at 4800 MT/s). Case: `copper
synthesize-antenna examples/antenna_synthesis/board.copper --antenna bt
--verify`, the 22.66 mm monopole then pinned on the 40 x 30 mm board (model
revision 2). Nothing here
changes the [qualification boundary](engineering-qualification.md).

`COPPER_ANTENNA_PROFILE=1` prints a mesh census and the run's timing;
`COPPER_ANTENNA_PROFILE=mesh` stops after compiling (cells and time step in
about a second); `lines` adds every cell below 0.45 x the cell.
`COPPER_ANTENNA_FINE_CELL=0.125` overrides the fine cell for convergence
references. The experiment switches used below (`COPPER_ANTENNA_X_*`) were
removed when model revision 3 landed.

## Baseline

| Fidelity | Cells | Steps | dt | Simulated | Run (6 threads) | Resonance | Bare / matched | Efficiency |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| coarse | 1.375 M | 14,051 | 0.282 ps | 3.97 ns | 55-87 s | 2.3881 GHz | -3.27 / -12.73 dB | - |
| fine | 2.520 M | 30,801 | 0.149 ps | 4.59 ns | 214 s (12 threads) | 2.3867 GHz | -3.13 / -12.05 dB | 0.852 |

Both runs converge on energy decay long before the 25/40 ns caps. Mesh
generation and operator compilation take 1 to 3 s: all the time is the
time loop, so cost = cells x steps / update rate.

Thread scaling (coarse; results bit-identical at every count):

| Threads | 1 | 2 | 4 | 6 | 12 (the current default) | 3 runs x 4 threads at once |
| --- | --- | --- | --- | --- | --- | --- |
| Run | 119 s | 104 s | 67 s | 55 s | 96 s | 144 s wall, 48 s per run |
| Mcell-updates/s | 162 | 186 | 289 | 349 | 202 | 3 x 137 |

At about 100 bytes moved per cell update (6 field and 12 coefficient f32
values read, 6 written), 349 Mcell-updates/s is about 35 GB/s, roughly 60 %
of this machine's peak DRAM bandwidth. SMT threads cost 1.7x, and running
three evaluations at once gains only 15 % in throughput: the engine is near
the memory-bandwidth limit here. Wall time is noisy (the same baseline ran
in 55, 67 and 87 s across the session), so cells x steps is the primary
metric and wall time is quoted with its spread.

## Findings

1. **The default thread count is the slowest sane choice.** `--threads`
   defaults to `available_parallelism()` (12 logical CPUs); 6 physical cores
   are 1.74x faster.
2. **The board is meshed much finer than the fidelity asks.** The coarse
   cell is 0.5 mm (two across the 1 mm track), yet the board's 40 x 30 mm
   carries 123 x 75 cells (0.33 / 0.40 mm average) and 141 of the 295
   in-plane cells are below 0.3 mm. The extra lines are csx's detected
   edges of every primitive (0.4 mm stitching-via boxes, ground polygons,
   slabs, the port) plus its 1.3-ratio grading between nearby fixed lines,
   which produces cells below `min_res`. The z axis has 8 cells of 0.17 to
   0.21 mm across the 1.51 mm core where the fidelity asks for 3.
3. **A sliver sets the time step.** The smallest cells are 0.209 / 0.112 /
   0.170 mm (x/y/z) on the coarse mesh, the y one at the feed and slot
   (y 6.0 to 7.6 mm, eleven cells of 0.11 to 0.16 mm); the fine mesh has
   0.069 / 0.062 mm cells against a 0.25 mm fine cell. `dt` follows the
   smallest cell per axis, so these halve the step.
4. **Merging lines is not a fix.** Raising `min_res` from 0.45 to 0.6-0.8
   of the cell removes 33-54 % of the cells and raises `dt` 1.2-1.6x, but it
   moves real edges: the coarse resonance falls from 2.388 to 2.222 GHz
   (-7 %), the fine one to 2.344 GHz (-1.8 %). Geometry-defining edges must
   stay exact; only the incidental lines may go.
5. **95 % of the cells are outside the board's footprint**: the board's
   fine x/y lines extruded through 58 z-lines of air, and its fine z lines
   through the air beside it. A lambda/8 air margin instead of lambda/4
   (for S11) removes 27 % of the cells with identical results: 2.3881 GHz,
   -3.27 / -12.73 dB coarse; 2.3867 GHz, -3.13 / -12.06 dB and efficiency
   0.853 fine. The far-field box still fits.
6. **The per-cell cost depends on the mesh.** With the lambda/8 margin the
   update rate fell (135 vs 162 Mcell-updates/s on one thread; 142-150 vs
   222-290 at 6 threads, where it ran slower overall despite fewer cells;
   at 3 threads it was 23 % faster as expected). A larger PML share and the
   engine's thread partitioning are the suspects; unexplained until
   profiled.
7. **Free-space air cells alone are a small win** (12 % fewer cells) and,
   as prototyped, add a 0.158 mm sliver and shift the coarse resonance by
   10 MHz; they belong in the mesher rework, not as a switch.
8. **The Rennings time-step criterion gains nothing** (0.282 to 0.286 ps):
   the limit is geometric, not material.
9. **Conformal PEC does not apply**: `ems` resolves solid metal and off-grid
   sheets; our copper is zero-thickness sheets on mesh planes, which stay
   staircased.
10. **The pixel search can pay for identical copper.** Its cache key is the
    raw bit string (`Search::run`, `geometry.to_source()`), while `layout()`
    realizes only tiles 4-connected to the feed, so a mutation of an
    unconnected tile costs a full evaluation of unchanged copper. The share
    is not yet measured.

## Model revision 6

A mesh line keeps every role it carries through merging: a copper edge
that lands on a port line is still copper, so a ground boundary (a feed-slot
wall at the board clearance) beside it keeps its own line. Before, the merged
line carried only the port rank, the wall merged onto it, and the slot wall
landed on the stub's copper and shorted the port.

## Model revision 5

Found synthesizing CopperAssetTracker's three antennas on JLCPCB's
JLC06161H-3313 six-layer stack (0.0994 mm prepregs under F.Cu and over
B.Cu, 0.1 mm board clearance); neither failure shows on the two-layer
example board:

- **Thin prepregs.** z lines closer than half the z cell merge, and equal
  ranks keep the first, so F.Cu's line (z = 0) merged into In1.Cu's
  (-0.0994 mm): the top ground and radiator moved onto the inner plane, and
  the grading around the lost line left a 0.0000 mm cell (150 z lines, dt
  0.0000 ps, which never finishes). The z snap is now at most 0.45 x the
  thinnest dielectric, so every interface keeps its own line: 57 z lines and
  dt 0.20 ps for the BLE site's fine mesh (1.32 M cells).
- **Ground boundaries next to copper.** Region and slot edges (where the
  ground sheets end) ranked below copper, so a ground edge closer to the
  radiator than the snap distance merged onto its line and shorted the port:
  the feed slot's sides at the 0.1 mm board clearance (Z = j1.8 ohm at every
  frequency, 0.0 dB at every length), and the region's start 0.25 mm from a
  1.5 mm stub, whose edge was also dropped for the nearby port line (Z =
  j3 ohm). A ground boundary and a copper edge, or two ground boundaries,
  now never share a line (a sheet edge whose line was dropped is snapped by
  the solver onto the nearest line, which was the stub's when the slot side
  stood 0.11 mm past the region start); a
  copper edge keeps its own line beside a port line unless within a quarter
  cell of it, where it moves onto the port (the stub's square end inside the
  port gap would otherwise leave a sliver that sets the time step). Only the
  region's padding still snaps onto copper. The slot keeps the board
  clearance at its sides, as the fill will.

These change what is simulated, so the revision moves and every `tuned_for`
digest is renewed by a fresh synthesis. The example re-synthesized to a
22.77 mm monopole (height 5.3 mm) at 2.371 GHz on the fine mesh, -12.3 dB
matched over the band with 0.85 nH series then 2.54 pF shunt, 86 %
efficiency, against 22.73 mm under revision 4 (+0.2 % in length). Keeping
the real gaps costs time step: the example's fine dt is 0.14 ps (0.24 ps
in revision 3), since the 0.1 to 0.2 mm ground gaps beside the stub are now
cells of their own.

## Model revision 4

No cell is coarser than the frequency limit (a fifteenth of the guided
wavelength at the sweep's top): the fidelity cell, and so the feed and
short zones that refine from it, are capped there. Before, the zones kept
the fidelity cell, so bands above about 12 GHz on FR4 got cells coarser
than the limit (0.75 mm against 0.416 mm at 24 GHz in er 4). Below that the
mesh is unchanged, but the revision still moves so every `tuned_for` digest
is renewed by a fresh synthesis.

## Model revision 3

**Thread default.** `--threads` defaults to one solver thread per physical
core (`model::auto_threads`: the solver's own rule, with half the logical
processors on Windows, where it cannot count cores without `unsafe`). The
example picks 6 and runs in 54.9 s where the old default (12) took 96 s,
with identical results.

**Search cache.** The cache keys on realized copper. A budget-12 pixel search
on the example board (ground on `B.Cu` only) skipped 3 of 15 candidates as
copper already evaluated: a quarter of the budget the old key would have
spent on duplicates. Duplicate children no longer enter the population, and
10,000 duplicates in a row end the search (they cost no budget).

**The mesher** (`axis_lines`) chooses every line; csx detects none:

- Port terminals and the line between them never move; radiator, feed land,
  short and via edges outrank region, slot and padding lines, which outrank
  ground vertices and the outline. Lines closer than half a cell merge into
  the higher rank (half a zone's limit inside a finer zone).
- The antenna's features are meshed at the fidelity cell; 1 mm around the
  port and around an inverted-F's short, at 0.75 of it.
- Gaps inside the zones are split into equal cells; `smooth_lines` grades
  only outside them (inside, its 1.3 ratio rule split a 0.3 mm cell next to a
  0.5 mm one into slivers).
- Stitching vias are PEC wires on the nearest mesh lines. Without them the
  coarse resonance falls to 2.329 GHz and the ring-down takes twice the
  steps (the parallel-plate cavity), so the wires do act as vias.
- An eighth of a wavelength of air before the PML.

What the profile and the experiments showed on the way:

- The radiator's 1 mm track starts at the centre of the 1.0 x 0.6 mm feed
  land with a square end and reaches 0.2 mm into the 0.5 mm port gap; ems
  drops the port edges that lie on metal. With about two coarse cells across
  the gap, the result depended on how many incidental lines crossed the port
  (five in the revision 2 mesh; removing them moved the coarse resonance to
  2.222 GHz). Revision 2's 1 MHz coarse/fine agreement was partly luck.
- The z axis is not the sensitive one: halving the coarse z cell moved both
  antennas down by 0.6-1 %, away from the references.
- An inverted-F's short (0.6 mm lands under a 1 mm leg) needs the feed's
  resolution: without its zone the coarse inverted-F read 2.7 % low.
- With edges snapping onto region lines, a 0.07 mm height change could widen
  the track by 0.2 mm and the coarse resonance jumped 34 MHz; ranking copper
  above region lines made the coarse response to height monotonic.

### Accuracy against converged references

References are fine runs at 0.125 mm (track / 8). The monopole has three
(revision 2 mesh, an intermediate mesh, revision 3): 2.3719, 2.3661 and
2.3700 GHz, a 0.25 % spread; the inverted-F one (revision 3): 2.8053 GHz.

| Case | Revision 2 | Error | Revision 3 | Error |
| --- | --- | --- | --- | --- |
| monopole 22.66 mm, coarse | 2.3881 GHz | +0.7 to +0.9 % | 2.3949 GHz | +1.0 % |
| monopole 22.66 mm, fine | 2.3867 GHz | +0.6 to +0.9 % | 2.3822 GHz | +0.5 % |
| inverted-F template, coarse | 2.8125 GHz | +0.26 % | 2.7920 GHz | -0.47 % |
| inverted-F template, fine | 2.8118 GHz | +0.23 % | 2.8117 GHz | +0.23 % |

Return loss stays within 0.25 dB of the references and efficiency within
0.011. The fine fidelity, which decides the result, is as accurate as or
better than revision 2's. The coarse fidelity is the search's proxy: it
reads 0.5-1.0 % above the fine one across the monopole geometries tried
(heights 5.0 to 5.3 mm, lengths 22.4 to 22.66 mm), smoothly. Ranking copper
above region lines cost the coarse monopole its earlier -0.05 % (a 26 MHz
shift) in exchange for that smoothness. Fine verification and its length
correction remain the safety net. The AN043 rebuild of the research note is
not in the repository and has not been re-run on revision 3.

### Cost

| Case | Revision 2 cells / dt / cell-updates | Revision 3 cells / dt / cell-updates | Less work |
| --- | --- | --- | --- |
| monopole coarse | 1.375 M / 0.282 ps / 19.3 G | 0.294 M / 0.477 ps / 2.4 G | 8.0x |
| monopole fine | 2.520 M / 0.149 ps / 77.6 G | 0.721 M / 0.243 ps / 13.6 G | 5.7x |
| inverted-F coarse | 1.546 M / 0.269 ps / 22.3 G | 0.386 M / 0.437 ps / 3.4 G | 6.6x |
| inverted-F fine | 2.695 M / 0.185 ps / 64.4 G | 1.025 M / 0.222 ps / 20.3 G | 3.2x |

The whole example synthesis (12 coarse evaluations, fine verification and a
length correction) took 3 min 21 s; on revision 2 the same work was about
18 minutes at 6 threads (12 x 55 s coarse plus two 214 s fine runs). Wall
times on this laptop vary up to 4x between identical runs (69 to 375
Mcell-updates/s for the same mesh), so cell-updates are the comparison; the
coarse model now holds about 21 MB of fields and coefficients, close to the
16 MB L3.

The re-synthesized example is a 22.73 mm monopole (height 5.3 mm) at
2.376 GHz, -12.2 dB matched over the band with 0.88 nH series then 2.57 pF
shunt, 86 % efficiency. `site/assets/antenna-synthesis.json` still shows the
revision 2 numbers and needs its promo build re-run.

## Plan

Each step is one pull request, measured on the ladder below before and
after, and must pass the accuracy gate. Gains marked *estimate* are not
measured yet.

### Ladder and gate

- Cases: the monopole (`board.copper`), the inverted-F
  (`inverted_f.copper`) and the AN043 rebuild, each coarse and fine with
  `--verify`; one pixel search on a small grid with a fixed budget.
- Metrics: cells, steps, dt (deterministic); wall time at 1, 3 and 6
  threads, best of two, mains power, recorded with the CPU and memory speed.
- Gate: resonance within 0.5 % of the current model, bare and matched worst
  case within 0.3 dB, efficiency within 0.01, coarse/fine resonance
  agreement no worse than today; `cargo test -p copper-antenna --release`.
  A change that alters the model bumps `MODEL_REVISION` and refreshes the
  examples' `tuned_for` digests.

### 1. Free wins (small)

1. **Done.** Default `--threads` to the physical core count (measured 1.74x
   on this machine against the old default).
2. **Done** (with step 2). Lambda/8 air margin for coarse and fine (27 %
   fewer cells, results identical). The update-rate drop of finding 6 is
   still unexplained (step 3.1).
3. Not planned: `-C target-cpu=native`. Measured at 55.5-56.1 s against
   56.5-57.2 s generic (6 threads, two runs each, identical results): no
   gain, as expected of a bandwidth-bound loop.

### 2. Mesh rework (the large one): done, model revision 3

See [Model revision 3](#model-revision-3) for what landed and how it measured.
The plan as written:


Replace "our lines plus csx detection and smoothing" over the board with a
mesher that owns every line inside the board's footprint:

- Geometry-defining edges (radiator, feed land, slot, ground edge, region,
  outline, short leg and via) are exact fixed lines, never merged.
- Stitching vias are snapped to existing lines and sized to one cell, so
  they add none; the board stops contributing csx-detected lines (restrict
  detection or pass the complete line set).
- Uniform fidelity cells over the radiator and a few millimetres around it,
  graded to lambda/(15 sqrt(er)) across the ground; no cell below 0.45 x the
  fidelity cell unless two real edges are closer, which the census reports.
- z: the fidelity's cells across each dielectric, then graded air; air cells
  up to free-space lambda/15 outside the footprint.

*Estimate*: about 90 x 65 x 30 cells for the coarse case instead of
173 x 124 x 66 (roughly 8x fewer) and a 1.5x larger dt, so up to about 10x
less work per evaluation; the fine mesh gains similarly. This is where the
accuracy gate matters most; findings 4 and 7 show how quickly a careless
merge moves the resonance.

### 3. Engine throughput (investigation, likely upstream in `ems`)

1. Profile the time loop (sampling profiler) on the 6-thread lambda/8 case:
   PML cost per cell, how the grid is split between threads, the energy
   check interval. Explain finding 6.
2. Coefficient compression: most cells share one material, yet each stores
   12 coefficients. openEMS's compressed engine keeps one coefficient set per
   distinct material combination; since the loop is bandwidth-bound, cutting
   about 100 to about 40 bytes per update is worth up to 2x (*estimate*).
   Contribute to `ems` (MIT) or carry it with `[patch.crates-io]` against the
   `=0.0.5` pin until a release has it.

### 4. Fewer evaluations

1. **Done.** Pixel cache key on the realized copper, with a log line
   counting skipped duplicates (finding 10).
2. Batch candidates: a generational pixel GA and a parallel pattern search
   for the parametric families. On this laptop three concurrent runs gain
   only 15 %, so this pays on many-core or high-bandwidth machines (or after
   step 3 lowers the bandwidth per update), not here.
3. Not planned: early stopping by signal extrapolation. Runs already stop
   at about 4 ns, with little ring-down left to cut.

### 5. Hardware, after 2 and 3

Re-measure Mcell-updates/s and bytes per update after the mesh and engine
work. The coarse model holds about 99 MB of fields and coefficients today;
at step 2's estimate (about 175 k cells) it would be about 13 MB, inside
even this laptop's 16 MB L3, and the update rate should then rise well
above the DRAM-bound figures above. Larger models fit an AMD X3D cache
(96 MB); a few hours on a cloud EPYC (Genoa-X) instance test that before
any purchase.

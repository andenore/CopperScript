# Routing review implementation and verification

Review baseline: `ddfa9b2`, full-vertical six-layer board, 100 x 80 mm.
Independent KiCad baseline: 74 unconnected items, 68 dangling tracks,
eight footprint-library findings. Earlier substantially connected board:
one GND unconnected item and eight library findings.

This list tracks every finding from the 2026-10-01 routing review.
Completion means tested implementation, not automatic manufacturing signoff.

## Correctness first

- [x] R1: Scope fanout cleanup to attempted nets; preserve unrelated copper,
  empty subsets, failed-subset rollback, and pre-existing/shared vias.
- [x] R2: Check complete candidate connectivity during local/placement repair,
  reject new ordinary-net opens, preserve unaffected geometry, and recompute
  committed metrics instead of trusting stale per-net flags.

## Route geometry and costs

- [x] R3: Preserve 45-degree reachability when pad coordinates split search
  axes; retain exact clearance checks and test split-axis invariance.
- [x] R4: Normalize global/detailed layer and wrong-way costs by physical
  length; test split-edge and routing-pitch invariance.
- [ ] R5: Bounded improvement of feasible routes: compare wider/projected
  guides for expensive incumbents, without sacrificing connectivity or DRC.
- [ ] R6a: Add clearance-aware local demand and failure pressure so spare
  signal layers can relieve congested channels; keep GND plane reserved and
  retain explicit layer/reference restrictions.
- [ ] R6b: Localize neutral-cost fallback and report effective policy rather
  than silently replacing the whole board with an unreported neutral rerun.
- [ ] R7: Branch-safe straight/45-degree cleanup for multi-terminal trees,
  including orthogonal-first/budget-fallback routes; protect pads, junctions,
  vias, and exact clearance. Improve topology without breaking existing trees.
- [ ] R8: Exercise critical USB/RF profiles in the full-vertical example;
  reserve critical geometry and protect paired nets in repairs. Do not infer
  controlled impedance or invent electrical limits from names.

## Matched rerun and repeated review

- [ ] Run focused regressions and the full available test suite.
- [ ] Reroute the same full-vertical board/placement and settings, documenting
  any explicit critical-profile or option changes separately.
- [ ] Refill zones and run independent KiCad DRC on disposable artifacts.
- [ ] Render all six layers and central hotspots; repeat length, density,
  heading, bend, via, detour, and actual-connectivity measurements.
- [ ] Compare against both the damaged latest and earlier connected board;
  never credit copper deletion as wirelength/via optimization.
- [ ] Record remaining bottlenecks and use exact access contributors for
  bounded placement moves/45-degree rotations if required; preserve functional
  decoupling/RF/power clusters rather than spreading components indiscriminately.
- [ ] Commit verified feature changes, leaving user-owned draft files alone.

## Acceptance gates

Unchanged nets retain their copper and connectivity. A repair introduces no
new ordinary-net open or hard DRC regression. GND-zone deferral is separate
from signal connectivity. Independent filled-zone KiCad verification remains
mandatory. All production, return-path, footprint qualification, and CAM
requirements remain separate from route completion.

## First implementation pass

R1–R4 are implemented with focused regression tests. Subset cleanup filters
its access mapping, requires explicit via ownership before removing input vias,
and preserves fanout on failed repairs. Local acceptance checks unaffected
copper identity and fresh native connectivity. Merge metrics are measured from
committed geometry, and placement/detailed scoring counts actual ordinary-net
opens. Diagonal successors skip inserted coordinate splits. Shared integer
costs use a 1 mm physical reference, independent of routing pitch; fixed bend
and via events remain separate. The matched rerun below determines actual
board quality; R5–R8 remain open for subsequent implementation passes.

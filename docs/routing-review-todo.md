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
- [x] R3b (follow-up review): Replace index-space supercover sampling with
  physical-space obstacle/outline sampling on nonuniform rays. An off-ray
  blocked node can still conservatively suppress a restored diagonal edge.
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
- [ ] R9 (follow-up review): Record phase timings/checkpoints for long full
  reruns and cache equivalent physical-span via legality queries within one
  immutable search. A 15 s/734-sample profile during zone-escape feedback
  attributes 27.4% of samples to via checks, 13.1% to track checks, and 12.8%
  to neighbor generation (inclusive categories; not whole-run timing).
  Search-local physical-span via caching is implemented in pass 2; operational
  timings/checkpoints are still outstanding, so this item remains open.
- [x] R10 (rerun finding, correctness priority): Replace endpoint-only native
  connectivity with layer-aware copper-contact connectivity. Detect interior
  T/cross junctions, track/via overlap, and pad-shape contacts; retain physical
  via spans and independently verified zone evidence. Native checking flags
  V3V3, V3V8, MODEM_EN, and PWR/MODEM_FB as open on a board where KiCad finds
  none of those opens. An interior-T regression reproduces the defect. Do not
  waive open-net checking or assume every rejected placement trial was valid.
- [ ] R11 (rerun finding): Resolve U_CC.3/.10/.11 GND as a joint package-access
  group, reserve its escapes before ordinary routing, and evaluate bounded
  component/decoupler moves only after R10 makes trial acceptance reliable.
  Keep small-pad via drill/clearance rules and explicit fabrication limits.
  Pass 2 implements automatic whole-package same-zone-net reservations;
  actual filled-zone closure of the example must still be verified.

## Matched rerun and repeated review

- [x] Run focused regressions and the full available test suite.
- [x] Reroute the same full-vertical board/placement and settings, documenting
  any explicit critical-profile or option changes separately.
- [x] Refill zones and run independent KiCad DRC on disposable artifacts.
- [x] Render all six layers and central hotspots; repeat length, density,
  heading, bend, via, detour, and actual-connectivity measurements.
- [x] Compare against both the damaged latest and earlier connected board;
  never credit copper deletion as wirelength/via optimization.
- [x] Record remaining bottlenecks and exact access contributors for subsequent
  bounded placement moves/45-degree rotations. See R11; preserve functional
  decoupling/RF/power clusters rather than spreading components indiscriminately.
- [x] Commit verified feature changes, leaving user-owned draft files alone.

## Acceptance gates

Unchanged nets retain their copper and connectivity. A repair introduces no
new ordinary-net open or hard DRC regression. GND-zone deferral is separate
from signal connectivity. Independent filled-zone KiCad verification remains
mandatory. All production, return-path, footprint qualification, and CAM
requirements remain separate from route completion.

- [ ] Clear remaining filled-zone GND opens (R11); do not claim routing complete.
- [ ] Qualify footprints and resolve the eight independent library findings;
  none were suppressed or fixed by this routing pass.
- [ ] Complete critical-net, return-path, fabrication and independent CAM
  signoff before claiming this example is production-ready.

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

Commit: `8b8c896`. All 277 tests pass in an isolated Python 3.12 test runtime
with pinned `pygerber` installed (459 upstream deprecation warnings). This
includes eight new review regressions. The post-commit focused acceptance/flow
rerun passes 16 tests. Independent full-board results are recorded separately.
The initial Python 3.13 run passed 275 tests but lacked the optional CAM
dependency; the Windows Store Python launcher also cannot supply the readable
executable-byte identity required by the CAM adapter. No CAM check or
executable-identity requirement was weakened to work around that environment.

The repeated review is in [routing-review-pass1.md](routing-review-pass1.md).
Independent KiCad finds three GND unconnected items, zero dangling tracks,
and eight library findings, versus 74/68/eight on the damaged baseline. All
57 ordinary nets are connected under KiCad. The earlier best board still has
fewer ground opens (one). At the end of that pass, R10 was the next correctness
task; R3b and R5–R11 were unfinished.

## Second implementation pass

R10 and R3b are implemented. Native connectivity uses exact rounded-shape
contacts on common physical layers, including annular/pad-edge contacts and
interior junctions; it preserves actual gaps and drill voids and does not infer
filled zones. Static ray checks follow physical coordinates and exact polygon
containment for both diagonal and orthogonal moves. R11's whole-package
same-zone-net reservation is implemented; actual example ground closure remains
an independent acceptance gate. R9's search-local physical-span via cache is
implemented, while normal operational timings/checkpoints remain open.

All 297 tests pass in the isolated Python 3.12 runtime, including 20 new tests
since pass 1 and five optional installed-KiCad differential connectivity cases.
Replaying the previous board's copper clears all four false ordinary-net opens;
only GND remains open under native explicit-copper checking. No DRC check was
waived and no geometry/fabrication limit was relaxed. Full-board rerun/review
results are recorded separately once independent verification completes.

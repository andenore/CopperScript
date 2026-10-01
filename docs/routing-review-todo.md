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
  - [x] R8a: Declare explicit profiles; atomically validate critical candidates
    against native geometry/connectivity; protect pair copper through subset
    repairs and duplicate-land closure. Real-footprint preflight connects four
    RF nets with no vias and rejects both unsafe USB pairs. See
    [pass 4](routing-review-pass4.md); this does not complete R8.
  - [x] R8b: Joint same-layer paired package-access search. Enumerate legal
    paired exits, preserve terminal order, search a heading-aware common spine,
    and materialize clearance-checked straight/45-degree corners/tapers.
    Bounded pitch refinement connects both real-footprint USB pairs without
    vias or placement changes. Independent KiCad confirms no critical opens
    or hard copper findings. See [pass 5](routing-review-pass5.md).
    New paired layer transitions and non-octilinear/multi-terminal pairs remain
    unsupported by this search, not silently approximated. Existing transition
    candidates still require atomic native/profile/return-via acceptance.
    Future profiles needing transitions require their owning paired search;
    package-placement feedback follows if the bounded surface search fails.
  - [ ] R8c (next): Enforce RF reference-layout/matching clusters and antenna keepouts;
    shorten the Nordic and GNSS paths. Connected long top-layer feeds are not
    evidence of RF performance, matching-ground topology or return continuity.
    - [x] Audit electrical terminals/topology first: replace the generic antenna
      with the source-backed Johanson part, isolate its NC anchor, and move the
      Nordic 0.8 pF shunt to the chip side of the 3.9 nH series inductor.
      Evidence and ERC regressions are in CopperLib/CopperScript.
    - [ ] Complete the Nordic support circuit and extract a pin/pad-bound rigid
      matching/reference-ground cluster from the vendor layout, not a guessed
      distance threshold. Preserve its legal translation/rotation transforms.
    - [ ] Implement antenna corner placement and qualified local layer-aware
      copper/ground keepouts without deleting its isolated mechanical land.
    - [ ] Optimize receiver/connector RF clusters and noise-source separation;
      validate the new placement and reroute critical groups transactionally.
      See [RF audit](rf-layout-audit.md). R8c remains open.
  - [ ] R8d: After critical preflight passes, perform the complete rerun and
    independent KiCad layer review. Bind actual impedance/return-path evidence
    to the selected physical stackup before any critical-net signoff claim.
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
- [x] R11 (rerun finding): Resolve U_CC.3/.10/.11 GND as a joint package-access
  group, reserve its escapes before ordinary routing, and evaluate bounded
  component/decoupler moves only after R10 makes trial acceptance reliable.
  Keep small-pad via drill/clearance rules and explicit fabrication limits.
  Pass 2 implements automatic whole-package same-zone-net reservations;
  independent KiCad refill confirms zero unconnected items in pass 2, without
  moving components or relaxing fabrication limits.
- [x] R12 (pass-2 review): Evaluate duplicate-land closure consistently before
  feedback scoring and final reporting. USER_BUTTON is checked before final
  SW_USER land stitching; the finished board is connected. Reconcile pending
  shield-land helpers with digest-bound filled-zone evidence without inventing
  virtual contacts or suppressing footprint/manufacturing findings.
  Implemented in pass 3: shared exact land roots, pre-scoring closure, measured
  additions/status, and explicit surface-pending versus verified-zone reporting.
  Fresh saved-board KiCad evidence resolves both shield references without
  changing its copper; 313 tests pass. The matched rerun is recorded below.

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

- [x] Clear remaining filled-zone GND opens (R11). Pass-2 independent KiCad
  reports zero unconnected items. This is connectivity, not production signoff.
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
same-zone-net reservation is implemented and the example's ground closure is
independently verified. R9's search-local physical-span via cache is
implemented, while normal operational timings/checkpoints remain open.

All 297 tests pass in the isolated Python 3.12 runtime, including 20 new tests
since pass 1 and five optional installed-KiCad differential connectivity cases.
Replaying the previous board's copper clears all four false ordinary-net opens;
only GND remains open under native explicit-copper checking. No DRC check was
waived and no geometry/fabrication limit was relaxed. Implementation commit:
`4839b90`. The [second repeated review](routing-review-pass2.md) records the
matched rerun: zero KiCad unconnected items, zero dangling/short/clearance
findings, eight unchanged footprint-library findings. The accepted joint-package
trial retains every component position and rotation. R5–R9 and R12 remain open;
native zone deferral and library findings still prevent production-ready status.

## Third implementation pass

R12 closes duplicate physical lands before full and subset candidate scoring,
using the native exact-contact graph. Existing multilayer/pad-edge paths are
reused; bounded new bridges respect layer and width rules. Single-logical-pad
nets with separated physical lands now correctly fail native connectivity.
Search failure flags and zone deferrals remain failures; closure updates
connectivity, actual added ordinary copper, routing metadata and fingerprints.

Reports retain raw `surface_pending_pads` and identify `zone_verified_pads`
separately. Only fresh board/export-bound KiCad evidence with zero opens/islands
and no non-library violations resolves pending zone-net references. Eight
library findings remain failures, and no native finding or signoff token is
waived. Final-stage and selected-pipeline bridge counts are separate.

All 313 tests pass (16 additions since pass 2, 459 upstream warnings), including
installed-KiCad verification of a single-logical-pin duplicate-land open and its
closure. A read-only replay of the saved pass-2 board adds zero tracks, recognizes
four already-connected references, and resolves J_POWER.SH/J_SIM.SH through a
fresh independent refill while retaining all eight library findings.
Implementation commit: `c1d5e52`. The [third repeated review](routing-review-pass3.md)
records the completed matched rerun: zero KiCad unconnected items, no hard copper
findings, identical physical track/via/pad/placement geometry to pass 2, and
zero reported signal failures in the accepted feedback trial. Sixteen bridges
are now installed before scoring; final stitching adds none. Eight library
findings and native zone deferral remain signoff failures. R8 is the next
implementation priority; R5–R9 remain open.

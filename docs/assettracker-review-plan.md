# AssetTracker visual-review follow-up

Requested 2026-10-11. This is a checkable implementation plan, not a release
approval. Shared algorithms belong here; reusable RF circuits and reference
assets belong in CopperLib; CopperAssetTracker owns composition and evidence.
No board-local repair selectors or fabrication-rule relaxations are allowed.

## Baseline

- [x] Identify the reviewed artifact and compare it with the latest candidate.
  The five CopperReview comments refer to the historical 83-component board,
  SHA-256 `9e2b3e8ff432caec1918c3c33082bf7d0c52a8f777fd3a6c930117df5117400d`.
  The latest 91-component filled candidate has SHA-256
  `b63dd47ae84b9d9fea381420208dd6ffa113d89f1d9ecff8af07968907e3c3b1`.
  Preserve both artifacts and their exact source/DRC evidence.
  The modem escape comment concerns V3V8, not V3V3. All twelve narrow BAT
  segments in the newer candidate are permitted by its current breakout rules;
  improving preferred width is not repairing a declared width-rule violation.

## Implementation and acceptance

- [x] R1. Prefer full declared width for ordinary routes wherever exact copper
  clearance and the board outline permit it. Treat breakout width as a legal
  fallback, not an instruction to narrow all copper near a pad. Preserve wider
  accepted copper through cleanup. Test open space, tight pin fields, keepouts,
  board edges, existing narrow chains and unchanged critical-pair ownership.
- [ ] R2. Apply pad-aware straight outward departure/arrival preferences beyond
  greedy fanout. Test rotated/mirrored packages and blocked-normal fallbacks.
  Avoid cutting across adjacent same-net lands; distinguish deliberate common
  trunks/T junctions from gratuitous X crossings. Cover detailed routing,
  surface contacts and cleanup; do not rewrite immutable macro copper.
  - [x] Rank shared detailed/surface lead-ins by footprint-local outward
    departure and reversed arrival. Test front/back and 45-degree rotation,
    blocked-normal fallback, adjacent same-net lands and smoothing preservation.
  - [ ] Extend the preference to maze/topology decisions and explicit ordinary
    same-net X-crossing avoidance, preserving deliberate common trunks.
- [ ] R3. Minimize necessary neckdown extent using exact checked geometry, without
  extending declared breakout regions or claiming current-carrying qualification
  from trace width alone. Audit vias, pad contacts and the complete supply path.
- [x] R4. Improve bounded RF terminal access and route length selection. Reproduce
  the GNSS route (14,888 um for terminals 3,950 um apart) with a generic fixture.
  Keep RF layer/via/impedance contracts; do not invent a universal length bound
  or infer impedance compliance from a shorter route.
- [x] R5. Correct the existing CopperLib Johanson corner-antenna macro and its
  host-edge placement intent together. Ground clearance must open onto both
  required board edges while preserving vendor feed/matching returns. Read the
  exact reference layout; test rotations and keepout propagation on all layers.
  Review MAX-M10S integrated matching and LTE coexistence requirements separately.
- [ ] R6. Run a fresh source-only route of the 91-component tracker with standard
  shared CLI algorithms. Check exact-source readback, native filled copper/DRC,
  both USB pairs, every bypass association, supply contacts and the five reviewed
  regions. Retain prior 83-component release candidates; no custom repair script.
  - [x] Run the passive-GNSS RF revision with frozen compiler/library/source
    inputs; independently confirm source-bound readback and all 21 surface
    bypass connections. Both USB pairs and all 61 ordinary nets are routed.
  - [x] Close the remaining native V3V3 fill-fragment open and independently
    recheck the complete candidate using shared component-aware fill repair;
    no tracker-specific routing selector or waived native closure.
    - [x] Regress and implement electrical-component/layer-aware native-fill
      anchor sampling and geometry-bound rejection reuse in shared CopperScript.
    - [x] Freeze the corrected compiler and rerun the complete standard CLI;
      independently check the exact accepted board before claiming closure.
  - [ ] Visually recheck the five reviewed regions on the newly closed candidate.
- [ ] R7. Refresh review images, route metrics and README/evidence only after R6
  succeeds. Record exact input/tool/artifact hashes and remaining RF, USB, power,
  CAM, assembly and prototype gates. Zero opens is not production approval.

Record implementation results below; check off only demonstrated acceptance.

## Results

R1 is implemented in the shared exact-copper clearance index. It tries the
requested profile width first, including a full board-material check. If the
whole segment cannot fit, region-bounded fallback pieces independently retain
the full width wherever legal. Cleanup keeps checked wide copper rather than
narrowing it merely because a pad permits a smaller minimum. Existing valid
narrow chains remain reusable. Critical pair routing retains its explicitly
coupled neckdown behavior; no region, width or clearance declaration was relaxed.

The initial R2 implementation shares an outward-normal ranking derived from
footprint-local pad positions and body dimensions, transformed with rotation and
side. It is a preference, not a new electrical rule; central/through-hole lands
and conflicting coincident land directions have no inferred normal. Both ends
are ranked, and blocked straight candidates retain exact checked alternatives.
Ordinary smoothing cannot undo a better outward launch while removing bends.
This does not yet solve all ordinary-area crossings or redundant supply branches.

The new width tests failed against the original unconditional-neckdown behavior
before implementation. Older ordinary-width tests that demanded narrow copper
everywhere inside a region were updated to verify permitted minimums, full-width
large-land contacts, exact clearance and unchanged actual connectivity instead.

Focused validation: **373 distinct tests passed, zero failures or skips** across
four disjoint selections: 136 width/cleanup/native-feedback, 154 package/surface/
detailed-routing, 24 critical-routing and 59 quality/ownership/search-order tests.
These include real installed-KiCad CLI checks. JUnit reports are retained in the
chat workspace as `visual-review-width-native-20261011.xml`,
`visual-review-routing-20261011.xml`, `visual-review-critical-20261011.xml` and
`visual-review-quality-20261011.xml`. The six outdated width-policy assertions
in the initial mixed run were corrected and all eleven ordinary-neckdown tests
were replayed successfully in the 136-test selection. No full-suite or
tracker-reroute success is claimed.

The tracker canonical source/release lock hashes and the 91-component source and
filled-board hashes were rechecked and remain unchanged. No new manufacturing
package, source-matched 91-component reroute or production approval is claimed.

### RF preflight, 2026-10-12

R4: the measured GNSS detour passes the shared critical acceptance gate at
14,888 um. Cheap surface alternatives are blocked by actual lands; a bounded
octilinear surface maze yields an accepted 5,154 um, six-segment F.Cu candidate
with zero vias. The RF improvement stage now tries that existing shared search
for explicit no-via point-to-point RF profiles, without adding a maximum length
or changing width/clearance/impedance rules. An unusable any-angle cheap path
also no longer suppresses the octilinear fallback. Missing guides, original
budgets, macro copper and committed critical reservations remain acceptance
gates. This is a baseline geometry comparison, not the final rerouted board.

R5: the separate reusable CopperLib managed-footprint asset now declares v0.5
host-edge openings. CopperScript's typed placement contract rejects a closed
island, single-edge/partial opening, internal cutout and unsupported curved
perimeter; rotations to all four rectangular corners pass. The tracker RF
revision moves its Nordic macro and fixed support group together, opening the
6500 um square exclusion onto the top and right edges. The source-only 91-part
composition passes ERC and standard shared placement. An isolated KiCad refill
probe reports zero violations/opens; sampled matching/corner exclusions remain
empty on all six layers with positive fill controls and all 22 locked copper
objects preserved. No matching strokes, prescribed returns or exclusions changed.

MAX-M10S R05 source review confirms integrated input matching/filtering. External
GNSS antenna type, any bias/ESD/coexistence network, actual selected-stackup RF
impedance, ground/via-fence and enclosure/tuning/bench qualification stay open.
These checked items describe implemented algorithms and layout corrections,
not completion of those engineering or production gates. Full routing is held
pending the GNSS passive/active/bias choice; no full route was launched during
that preflight. The user subsequently selected a passive GNSS antenna, clearing
that prerequisite for R6 without adding an antenna-power bias circuit.

Validation: 165 distinct shared compiler regressions pass across the 163-test
selection and focused follow-up replays; CopperLib full suite 145 passed and
31 optional tests skipped; tracker composition/power tests 12 passed, zero
skips. Reports are `rf-generic-tests-20261012.xml`,
`rf-surface-tests-20261012.xml`, `rf-library-tests-20261012.xml` and
`rf-tracker-tests-20261012.xml` in the chat workspace. The complete compiler
suite was not run. RF review artifacts live under tracker
`build/rf-preflight-20261012/`; its copied compiler inputs are hashed separately.

One unlocked diagnostic refreshed the historical 91-part dependency lock.
It was recovered to the exact recorded closure-input SHA-256
`b9f99ccf343a78c859e0a4c3af7c64a2c3b7f87827b076bdf382b523819eea50`;
the diagnostic version is retained in the RF review directory. Later probes
copy the historical source scene before unlocked resolution. Canonical 83-part
source/release locks and the historical native PCB remain unchanged.

### Passive GNSS full run, 2026-10-12

The user selected a passive GNSS antenna. The tracker RF composition now records
that choice and includes no antenna-power bias circuit. Its composition/power
regressions pass: 12 tests, zero failures/skips, recorded in
`rf-passive-tracker-tests-20261012.xml` in the chat workspace.

The standard locked/offline six-layer `route-board` run is retained under tracker
`build/rf-passive-full-20261012/`, with frozen inputs under
`build/rf-passive-inputs-20261012/`. No historical copper or poses were imported,
no input changed during routing, and canonical/historical hashes were rechecked.
All 61 ordinary nets and both USB pairs are connected; actual GNSS feed copper
is 5154 um long on F.Cu with zero vias. A separate source/native readback confirms
91 components, six layers, 907 tracks, 318 vias, preserved source-owned macro
copper/required arrays, matching routing metrics and all 21 surface bypasses.

The run is **incomplete**, not a clean replacement or release: independent native
DRC reports zero violations and one open between fragments of the In3.Cu V3V3
zone. The bounded eight-trial feedback accepted a V3V8 bridge, reducing two opens
to one; the V3V3 proposals did not improve native connectivity. R6 and R7 remain
unchecked. Selecting the antenna type does not select a specific antenna/cable,
qualify RF/USB/power, close CAM/assembly/prototype gates or approve production.

Evidence: `route-rf-passive.json`, `run-inputs.json`, `run-result.json`,
`independent-checkpoint.json` and `independent-native-drc.json` in that run.
The retained filled candidate is `native-rf-passive/verification-005/`:
PCB SHA-256 `8f6269d94757eb95779e4331f007d84287bf8bf7242a00d8814de12fe96fed8c`;
route report SHA-256
`d22c6e755edd2e4a09966dcea2dcc24309e648e1172d85471717cf93031fb86d`.

## Native-fill component repair (12 October 2026)

The generic feedback controller now groups fresh native fill with the shared
bare-copper graph before sampling anchors. Separate polygons already joined by
tracks, plated transitions or actual fill do not receive redundant bridges.
Sampling shares its existing allowance across disconnected components and their
accessible layers, including when native DRC names a zone on the opposite layer.
No new via, source-zone edit, installed-component internal join or board-specific
selector is used. Native-rejected endpoint pairs are skipped only while their
involved geometry/fill and relevant routing layers remain unchanged; dependency
reroutes invalidate that cache on changes to any copper layer.

Regression evidence is 157 passing tests, zero failures/errors/skips:
`fill-components-tests-20261012.xml` (118) and
`fill-components-quality-tests-20261012.xml` (39) in the chat workspace. Four
cache/stale-fill/drilled-void checks were also rerun after the final conservative
cache review in `fill-components-cache-tests-20261012.xml`. The renamed real
KiCad three-fragment fixtures close in one trial while preserving prefix copper.
No search/trial bound was increased.

The first snapshot was superseded before routing started. The complete standard
CLI rerun passes under tracker
`build/rf-fill-components-full-v2-20261012/`, with immutable inputs under
`build/rf-fill-components-inputs-v2-20261012/`. All 61 ordinary nets, both USB
pairs and all 21 surface bypasses are connected. Shared fill feedback accepts
V3V3 on In3.Cu in its first trial, rerouting only SIM_IO/SIM_RST/SIM_VCC; a
subsequent V3V8 bridge on In1.Cu closes the remaining open. There are three
trials total, with no increased bounds, new source constraints or custom repair.

Separate exact-source/native readback and independent KiCad DRC pass: 91
components, six layers, 925 tracks, 326 vias, zero native opens/violations/islands,
matching route measurements and preserved source-owned macro copper/arrays.
GNSS_RF remains six F.Cu segments, 5154 um long, with zero vias. No routing input
changed; canonical board/locks and the prior clean candidate hashes are unchanged.
The accepted filled PCB is `native-rf-passive/verification-003/CopperAssetTracker.kicad_pcb`:
SHA-256 `90ad07b17c793ae4413d84f6a828e6c29596da587810363bcb52c50fa341f7f6`.
Run inputs SHA-256 `a1a36693344eff33ef8d27c632ac86b23609e3b0a10909c4455ad0adf8d1eeee`;
route report SHA-256 `20a4206a148f48cdd7192737e4090ec95cbd06a5cc0fa9488e97fd9204dea398`.
See `independent-checkpoint.json` and `independent-native-drc.json` in the run.

This closes the routing failure, not production approval. The five-region visual
review and refreshed figures remain pending, as do geometry-bound RF/USB/power
requirements, antenna/cable/ESD/coexistence selection, independent CAM, assembly/
CPL review, prototype measurements and explicit final release approval. Bare
explicit-copper DRC still reports three deferred zone observations; fresh native
fill resolves them. Electrical requirements remain blocked, not silently waived.

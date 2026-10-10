# CAM, RF and power qualification implementation

Requested 2026-10-10. Generic algorithms and command-line validation belong in
CopperScript; reusable part/circuit requirements belong in CopperLib. Projects
own only their selected stackup, operating scenarios, geometry bindings and
measurement evidence. Never copy compiler algorithms into a board project.

## Implementation checklist

- [x] Add content-bound, fail/incomplete/pass evidence and deterministic reports.
  Bind final native PCB, fabrication artwork, parts, stackup, operating inputs,
  tool identities and results. Changed inputs must invalidate prior evidence.
- [x] Add a final-native manufacturing audit command: checksums, layer inventory,
  drill/netlist parsing, pinned independent parser corpus and artifact agreement.
  Keep parsing/render agreement separate from electrical copper connectivity.
- [ ] Add independent vector copper connectivity and geometric fabrication checks.
  Cover clearance, annular rings, mask/paste and drill alignment. Unsupported
  primitives or absent profile capabilities are incomplete, never silently pass.
  - [x] Adapter-neutral polygon connectivity, contact/barrel annulus coverage,
    planar clearance, rectangular paste area ratio and mask-web algorithms.
  - [ ] Qualified Gerber polarity/drill/curve geometry extraction and complete
    fabrication-profile integration. Algorithms alone do not qualify artwork.
- [x] Add reusable RF/power contracts with source/revision/locator provenance,
  explicit unresolved obligations, and strict validated numerical inputs.
- [x] Add RF checks against actual native filled reference copper, including holes,
  signal-layer adjacency and route envelopes. Keep this separate from exported
  Gerber proof. Require exact fabrication stackup and solver-backed impedance.
- [x] Add conductor resistance/voltage-drop/loss and load-envelope calculations,
  including via barrels, capacitor droop and regulator input demand. Do not label
  resistance estimates as thermal/current-capacity or control-loop qualification.
- [ ] Add contract-driven component ratings, capacitance derating, inductor,
  switching-loop/feedback/thermal-layout checks and simulator evidence adapters.
  - [x] Strict mandatory design/simulation/bench contracts, numerical range/width/
    length/distance screens, sourced thermal estimate and byte-bound reviewed
    external solver/report adapters; no report-selected commands are executed.
  - [ ] Source/model-specific automated derating and switching-loop/feedback/
    thermal geometry extraction. Existing hard-macro ownership remains required;
    unresolved detailed review cannot be replaced by simple origin distances.
- [x] Integrate board-specific bindings and run checks on CopperAssetTracker's
  existing final-native six-layer candidate without rerouting or altering copper.
- [x] Add regression tests: broken/missing/stale evidence, plane gaps/holes,
  wrong reference layers, conductor neckdowns, invalid operating scenarios,
  parser disagreement and malformed manufacturing data.
- [ ] Connect all gates to release authorization; CAM/RF/power alone cannot approve
  a BOM, authorize an order or replace assembly/mechanical/battery review.
  - [x] Current-input `qualification-gate` and tracker ordering-target blockers.
  - [ ] Extend the older strict Physical-IR release API with final-native,
    six-layer evidence/profile support. The native file exporter remains
    unqualified; no production authorization or order endpoint was added.

## Verified initial implementation

Commands: `audit-cam`, `assess-engineering`, `qualification-gate`. Generic code
is in this repository; five reusable contracts are in CopperLib. Tracker inputs
and reports are in CopperAssetTracker. No copper/placement was changed.

The actual candidate passes all package hashes, 13 Gerber layer metadata, 295
drill hits, 552 physical contacts, 295 vias and net partition reconciliation.
Cellular/GNSS track envelopes pass filled In1 ground coverage with zero extra
margin (not impedance/return inductance qualification). CAM and RF/power overall
remain incomplete. External evidence is never inferred or falsely marked passed.

Focused compiler/native tests: 141 passed (no skips); reusable library/component
tests: 19 passed; tracker tests: 27 passed. These passes do not claim the full
suite passed. Rechecking the broader run's example failures produced six failures
and two setup errors: CM4/nRF52 example locks expect a different installed KiCad
footprint inventory, and full-vertical hard-macro asset bytes differ from their
recorded identities. Do not bypass the hash checks, rewrite those identities or
replace installed assets merely to turn the tests green.

- [ ] Reconcile the broader example asset/lock mismatches against their intended
  upstream content, preserving local changes; rerun the full suite afterward.

Latest tracker reports are `build/cam-audit-current.json`,
`build/engineering-audit-current.json` and
`build/qualification-gate-current.json`. All artifact/algorithm bindings pass;
overall status remains incomplete. Earlier dated reports are retained history,
not current evidence after native-probe/algorithm changes.

## External qualification checklist (cannot be invented by software)

- [ ] Select and obtain the exact fabricator stackup/capabilities; retain revision
  and reviewed copper thickness, dielectric thickness/Dk and finished plating.
- [ ] Provision and pin a second independently maintained CAM parser; qualify
  positive/negative, polarity, arcs, aperture macros, regions and slot corpus.
- [ ] Review exact SIM7670G-LNGV hardware-guide equivalence and antenna matching.
- [ ] Obtain effective MLCC capacitance and inductance/current/temperature data.
- [ ] Establish battery, USB, peak/RMS load and ambient-temperature envelopes.
- [ ] Run applicable impedance/SPICE/thermal solvers with pinned models/settings.
- [ ] Measure prototype antenna tuning/OTA behavior in the intended enclosure.
- [ ] Measure modem load bursts, low-battery operation, charging/NTC behavior and
  temperatures; review actual pack protection, harness ratings and polarity.
- [ ] Review every BOM/CPL selection and supplier preview; recheck stock at order.

Implementation progress, tested scope and remaining boundaries must be recorded
here. An unchecked item is not satisfied by native ERC/DRC or a generated report.

## Selected-supplier construction follow-up

- [x] Resolve and bind reusable supplier profiles from consumer engineering plans;
  reject consumer overrides and recheck profile bytes at the checkpoint.
- [x] Distinguish published construction from nominal finished thickness using
  sourced tolerance; do not stretch dielectric spacings or guess loss/minimum
  plating from published averages.
- [x] Inventory actual RF/USB widths and reference coverage. Isolated outer
  microstrip screening is auxiliary, never a replacement for external impedance
  evidence. No estimate is assigned to interior routes or unsupported references.
- [ ] Add qualified coupled/embedded/stripline/discontinuity solver integration.
- [x] Add reusable interface qualification contracts and external USB design,
  simulation and bench gates. Auxiliary pair geometry is not USB qualification.
- [x] Extract exact pad-centre/straight-track paths without shortest-path guesses
  on cyclic graphs; preserve branches, unused barrels and bounded return-via
  distances separately. Native contact layers/via type are retained read-only.
  Unsupported copper-edge/pad-area paths remain incomplete.
  Whole-channel USB contract/stage/coverage, path geometry, existing pair/stackup/
  RF-power/checkpoint/CAM and editor-overlay focused tests: 127 passed (2026-10-10).
- [x] Extract actual parallel pair gaps, unique covered length, member copper
  inventory/vias and adjacent plane nets/separations without assuming source
  gap, electrical delay or a ground reference. Auxiliary geometry only;
  focused pair/stackup/engineering/gate/CAM tests: 71 passed (2026-10-10).

CopperLib's JLC06161H-3313 companion profile is reusable. Tracker's selected-
stackup reports and BOM approval remain board-local; old artifacts are untouched.
Focused compiler checks and tracker regression counts are in the board's
`docs/stackup-selection.md`. Production qualification remains incomplete.

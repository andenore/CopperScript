# CopperScript roadmap

This list tracks the implementation sequence for routability-aware PCB
placement. A checked item means the behavior is implemented, documented, and
covered by automated tests; it does not mean the generated board has passed the
later Route or Verify gates.

## Placement engine

- [x] Expand the physical constraint model with courtyards, keepouts, placement
      regions, legal orientations, fixed/relative placement, alignment,
      component groups, and priorities. Lower typed `.copper` constraints into
      physical IR.
- [x] Build hierarchy-aware clustering from module provenance, explicit groups,
      interfaces, and proximity constraints. Preserve cluster membership and
      anchors in placement reports.
- [x] Replace component-center estimates with transformed physical pad
      coordinates. Add orientation-aware wirelength, per-layer crossing, escape,
      and routing-capacity metrics.
- [x] Add a deterministic analytical global-placement phase using smooth
      wirelength, density spreading, semantic group cohesion, and legal discrete
      orientations.
- [x] Add hybrid legalization: fast grid/Hanan-style legalization for ordinary
      rectangles plus bounded exact repair for small dense conflict regions.
- [x] Add detailed refinement with incremental moves, swaps, rotations, and
      periodic coarse-routing feedback while preserving all hard constraints.
- [x] Generate multiple deterministic candidates and return the Pareto frontier
      ranked by wirelength, crossings, congestion, constraint margin, and
      estimated vias. Expose candidate summaries through the CLI and JSON report.

## Completion criteria

- [x] All existing and new tests pass.
- [x] The full-vertical acceptance design produces deterministic, legal
      candidates.
- [x] Generated KiCad PCB files are accepted by the installed KiCad CLI when it
      is available.
- [x] Route remains `not_run` and Verify remains `blocked` until actual copper
      and sign-off implementations exist.

Verified on 2026-09-21 with the full test suite, byte-identical repeated
full-vertical artifacts, and KiCad 10 CLI parsing/DRC. The acceptance board
uses proxy footprints and has no routed tracks, so KiCad DRC violations are
expected and do not satisfy the future Route or Verify gates.

## Routing-to-manufacturing pipeline

- [x] 4. Add a deterministic multilayer congestion estimator/global router
      using capacity guides, multi-source A*, and negotiated congestion. Keep
      guide geometry separate from exact copper.
- [x] 5. Add a transactional placement–global-routing feedback loop with
      bounded legal movement, rollback, convergence control, and full-route
      certification.
- [x] 6. Add profile-driven critical-net routing for differential pairs,
      clocks, buses, RF feeds, and power routes before general routing.
- [x] 7. Add a guide-aware general detailed router with pin access, vias,
      exact tracks, deterministic rip-up/reroute, and cleanup.
- [x] 8. Add fail-closed physical DRC, coverage reporting, waivers, and signed
      signoff tokens tied to exact manufacturing geometry.
- [x] 9. Add gated manufacturing export, release manifests, checksums, and an
      independent CAM re-import/verification pass.

## Production-readiness program

Each item has its own research record and acceptance tests. Work proceeds in
dependency order; a checked item means its implemented scope is documented,
tested, integrated with the common physical IR, and exercised against KiCad
where relevant. It does not imply third-party certification.

- [x] P1. Copper zones and planes: typed zone/keepout IR, priorities, thermal
      relief and island policy; deterministic KiCad emission; content-bound
      fill provenance; and an authoritative, pinned KiCad refill before DRC and
      manufacturing. Native filling remains a preview feature until it passes
      differential tests against the qualified manufacturing filler.
- [ ] P2. Arbitrary-angle and push-and-shove routing: exact integer geometry,
      walkaround/hugging, deterministic shove transactions, rollback, acute-
      angle cleanup, locked-object behavior, and detailed-router integration.
  - [x] Exact integer predicates and rational squared-distance comparisons.
  - [x] Deterministic visibility-graph walkaround on expanded obstacle envelopes.
  - [x] Atomic recursive line shove with locked-track rollback.
  - [ ] Via/coupled-bundle shove, acute-angle cleanup, and detailed-router integration.
- [ ] P3. Differential-pair completion: coupled pad escape, fanout, via-pair
      transitions, uncoupled-length accounting, length/skew measurement,
      bounded trombone tuning, return-path requirements, and DRC.
- [ ] P4. Stackups and via technology: arbitrary copper/dielectric stacks,
      material properties, through/blind/buried/microvia definitions, legal
      spans, aspect/annular constraints, layer transitions, and KiCad mapping.
- [ ] P5. Exact-shape physical DRC: pad and track shape primitives, polygonal
      broad/narrow phases, exact clearance/intersection/connectivity, concave
      board edges, zones, stable markers, coverage, and differential checks.
- [ ] P6. Artwork, assembly, and fabrication DRC: solder-mask and paste rules,
      silkscreen clipping/clearance, courtyard and component-side checks,
      hole/slot constraints, copper balance and documented fab-profile gates.
- [ ] P7. Engineering analyses: stackup-driven impedance estimates, return-path
      continuity, conservative SI/PI checks, DC current/voltage-drop and thermal
      estimates, creepage/clearance profiles, explicit model validity and
      external-solver handoff without overstating signoff.
- [ ] P8. Independent CAM qualification: second-tool parsing/rendering and
      comparison, layer/drill/netlist reconciliation, polarity and extents,
      golden corpus, version-pinned qualification matrix, and release evidence.
- [ ] P9. Frontend and full-board closure: express every new rule in `.copper`,
      lower it into typed IR, resolve real CopperLib footprints, then place,
      route, DRC, export and independently verify the full acceptance board.

### Program completion criteria

- [ ] Research recommendation and source record exists for P1–P9.
- [ ] Every new geometry type participates in serialization, fingerprints,
      KiCad export, DRC coverage, and stale-signoff invalidation.
- [ ] Tests include unit geometry, adversarial regression, determinism,
      cross-stage compatibility, and negative release-gate cases.
- [ ] The full acceptance design completes without proxy footprints, unrouted
      nets, unwaived required DRC findings, or incomplete required coverage.
- [ ] A version-pinned independent CAM flow accepts the final manufacturing
      package and its evidence is recorded in the release manifest.

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
- [x] P2. Arbitrary-angle and push-and-shove routing: exact integer geometry,
      walkaround/hugging, deterministic shove transactions, rollback, acute-
      angle cleanup, locked-object behavior, and detailed-router integration.
  - [x] Exact integer predicates and rational squared-distance comparisons.
  - [x] Deterministic visibility-graph walkaround on expanded obstacle envelopes.
  - [x] Atomic recursive line shove with locked-track rollback.
  - [x] Atomic recursive via shove with locked-object rollback and detailed-router
        any-angle line-of-sight cleanup.
  - [x] Coupled-bundle shove and acute-angle cleanup.
- [x] P3. Differential-pair completion: coupled pad escape, fanout, via-pair
      transitions, uncoupled-length accounting, length/skew measurement,
      bounded trombone tuning, return-path requirements, and DRC.
  - [x] Atomic paired fanout, centerline offset routing, paired transitions,
        coupled/uncoupled measurement, skew and uncoupled-budget enforcement.
  - [x] Local bounded trombone tuning, return-via rules, and field-solver
        evidence binding.
- [x] P4. Stackups and via technology: arbitrary copper/dielectric stacks,
      material properties, through/blind/buried/microvia definitions, legal
      spans, aspect/annular constraints, layer transitions, and KiCad mapping.
  - [x] Ordered copper/dielectric stack, material properties, named through/
        blind/buried/microvia catalog, span/ring/aspect validation, and KiCad
        multilayer/via-kind mapping.
  - [x] Critical and detailed routers select a legal named technology from the
        same catalog consumed by validation and export.
- [x] P5. Exact-shape physical DRC: pad and track shape primitives, polygonal
      broad/narrow phases, exact clearance/intersection/connectivity, concave
      board edges, zones, stable markers, coverage, and differential checks.
  - [x] Shared exact integer segment/capsule predicates and exact copper-spacing
        decisions with deterministic markers.
  - [x] Shape-accurate rotated pads and normalized zone fills, spatial broad
        phase, incremental/full equivalence contract, concave-edge checks, and
        differential geometry checks. Copper arcs remain unavailable in the IR
        and therefore cannot be silently approximated.
- [x] P6. Artwork, assembly, and fabrication DRC: solder-mask and paste rules,
      silkscreen clipping/clearance, courtyard and component-side checks,
      hole/slot constraints, copper balance and documented fab-profile gates.
  - [x] Provenance-bearing process capabilities and separate fabrication,
        stencil, and assembly decisions for drill, mask-web, paste-area,
        courtyard, and height checks.
  - [x] Derived mask/paste/silkscreen geometry, slots, explicit edge-plating
        capability, orientation marks, copper balance, and release-gate
        integration. Unsupported artwork arcs make the process gate incomplete.
- [x] P7. Engineering analyses: stackup-driven impedance estimates, return-path
      continuity, conservative SI/PI checks, DC current/voltage-drop and thermal
      estimates, creepage/clearance profiles, explicit model validity and
      external-solver handoff without overstating signoff.
  - [x] Evidence-grade/status/scope/validity result contract, deterministic DC
        trace resistance and sourced creepage screening.
  - [x] Impedance/delay, return-path, via/DC network, thermal, and external
        solver evidence adapters.
- [ ] P8. Independent CAM qualification: second-tool parsing/rendering and
      comparison, layer/drill/netlist reconciliation, polarity and extents,
      golden corpus, version-pinned qualification matrix, and release evidence.
  - [x] Fail-closed qualification states, exact tool identities, immutable
        inventory hashes, safe input limits, and independent normalized-parser
        agreement contracts.
  - [x] Pinned PyGerber 2.4.3 adapter, isolated libgerbv CLI adapter, strict
        metric-XNC normalization, adversarial fixtures, and optional fail-closed
        publication binding.
  - [x] Reconcile drill multisets and IPC-D-356 net partitions to signed
        physical IR.
  - [x] Bind drill and IPC-D-356 reconciliation to CAM-required release;
        verify KiCad 10 Cartesian-up output, pad locations, and via records.
  - [x] Gate CAM-required release on a freshly run, hashed positive/negative
        corpus; record the corpus hashes in the release manifest.
  - [ ] Qualify an installed libgerbv build and expand the official golden
        corpus/matrix, including polarity and metamorphic render comparisons.
- [ ] P9. Frontend and full-board closure: express every new rule in `.copper`,
      lower it into typed IR, resolve real CopperLib footprints, then place,
      route, DRC, export and independently verify the full acceptance board.
  - [x] Normalized requirement/target/preference/assumption/external modes and
        fail-closed constraint consumer/verifier coverage.
  - [x] Constraint ownership metadata and critical routing profiles have
        concrete syntax and typed physical lowering.
  - [x] Content-addressed package lockfile and whole-board footprint audit.
  - [x] Complete the nRF52832-QFAA QFN48 bond map from the official Nordic
        pin table.
  - [x] Complete STM32G0C1RET6 LQFP64-GP bonds from ST DS13564 Table 12 and
        connect VBAT/VREF+.
  - [x] Correct the nano-SIM C7 I/O mapping and select an exact GCT connector;
        fail closed on unsupported embedded footprint keepouts.
  - [x] Replace the placeholder USB choke with orderable Coilcraft
        0603USB-601MLC and its verified pin-compatible KiCad land pattern.
  - [x] Preserve simple embedded footprint copper keepouts through import,
        physical IR, placement/routing checks, KiCad export, and signoff;
        installed KiCad audit now resolves 19/23 assets.
  - [x] Replace the virtual 5 V source with an orderable power-only USB-C
        receptacle and separate CC1/CC2 sink pull-downs; installed KiCad audit
        now resolves 20/24 assets.
  - [x] Use the Tag-Connect TC2050 bare-PCB SWD target for both processors;
        preserve KiCad connector contacts and footprint-local placement
        keepouts through import, placement checks, physical DRC, and export.
        Installed KiCad audit now resolves 21/24 assets.
  - [x] Generate the MAX-M10S-00B 18-land footprint and T-shaped stencil from
        u-blox UBX-20053088 R05 Tables 44-45; KiCad parses the artifact and
        the two-root footprint audit resolves 23/24 assets; the JLCPCB/EasyEDA
        EG800G-EU physical pattern is covered, but its full electrical model
        and Quectel mechanical/stencil signoff remain open.
  - [ ] Qualify the user's 5 V / 2 A input target with a measured worst-case
        power budget. Decide between a 5 V / 2 A USB-PD contract and a 3 A
        Type-C advertisement while limiting actual draw to 2 A; add input
        overvoltage, inrush, ESD, and reverse-current protection.
  - [ ] Resolve the reported real CopperLib/KiCad footprint gaps and complete
        acceptance-board placement, routing, DRC, manufacturing, and CAM closure.

### Program completion criteria

- [x] Research recommendation and source record exists for P1–P9.
- [ ] Every new geometry type participates in serialization, fingerprints,
      KiCad export, DRC coverage, and stale-signoff invalidation.
- [ ] Tests include unit geometry, adversarial regression, determinism,
      cross-stage compatibility, and negative release-gate cases.
- [ ] The full acceptance design completes without proxy footprints, unrouted
      nets, unwaived required DRC findings, or incomplete required coverage.
- [ ] A version-pinned independent CAM flow accepts the final manufacturing
      package and its evidence is recorded in the release manifest.

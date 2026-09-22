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
- [ ] 8. Add fail-closed physical DRC, coverage reporting, waivers, and signed
      signoff tokens tied to exact manufacturing geometry.
- [ ] 9. Add gated manufacturing export, release manifests, checksums, and an
      independent CAM re-import/verification pass.

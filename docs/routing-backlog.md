# Routing contributor backlog

This list tracks remaining algorithm scope, not past board failures. Verify an
implementation gap against current code and targeted cases before starting it.
Current contracts are [package access](package-access-first.md),
[boundary access](package-boundary-access.md), [filled-copper feedback](filled-copper-feedback.md)
and [placement](placement-internals.md). Board qualification stays in consumers.

## Access and incremental ownership

- [ ] Retain unaffected ordinary escape prefixes during the initial critical
  probe; the implemented partial rebuild after a disposable probe is distinct.
  Rebase occurrence-counted ownership and retain macro/required-array rollback.
- [ ] Extend joint domain allocation beyond alternate owner ordering to specialized
  paired/single-ended critical and dense power/GND access. Extend certified boundary
  domains and multi-bend candidates without breaking pair coupling or anchors.
- [ ] Rank complete access patterns with onward guide/layer congestion and permit
  bounded transactional replacement. Destination-facing ports are optional;
  guide-layer selection and verified via-chain collapse need further evaluation.
- [ ] Extend directional placement estimates to exact nearby-obstacle/access demand
  where the implemented pad-bank model is insufficient. Preserve whole units.
- [ ] Extend incremental repair to moved crowded packages/macros only with complete
  pad-level ownership and fresh joint access validation.

## Route quality and bounded search

- [ ] Improve feasible incumbents with bounded wider/projected guides and
  branch-safe octilinear cleanup; preserve junctions, pads/vias and owner scope.
- [ ] Extend exact same-net crossing preferences from package escapes to ordinary
  area topology while allowing intentional shared trunks and T junctions.
- [x] Add bounded joint placement selection for mutually blocked decouplers using
  [parameterized support patterns and complete contact/escape checks](power-area-planning-plan.md#pattern-based-support-placement-research-2026-10-17).
  Opt-in `--support-pattern-trials` retains fixed intent, complete local
  supply/return witnesses and ordinary reservations. Optimize support/contact
  envelopes, not just component bodies. Native/PDN qualification remains separate.
- [ ] Demonstrate the pattern search on an authored mutable CopperProbe bank,
  including onward D18 area access and all prior native return/contact groups.
  A separate four-component mutable candidate now passes the D18 subset on
  In3.Cu, with 112 returns and 15 critical nets preserved. Combined eleven-domain
  planning remains unresolved after 12 regeneration trials / 730 allocator
  states: sampled FX_USB18 blockers include trial-mutable ground-return, D18
  bypass and FX_V33 bypass copper. Exact area/contact feedback now repairs L9
  with fresh preservation of 112 returns, 28 ordinary ports and 15 critical
  nets. With seven neighboring areas held, FX_USB18 still has no complete tree.
  Sampled complete dependency closures span 362–391 occurrences / 76–88 source
  obligations. Implement retained shared trunks and exact continuation boundary
  witnesses before broader repair; keep the 64-occurrence / 12-obligation local
  release limits. Preserve the old incumbent and require combined native proof.
- [ ] Localize neutral-cost fallback and use clearance-aware demand/failure pressure
  while retaining explicit layer/reference restrictions and truthful policy reports.
- [ ] Reserve whole-bundle corner-wrap corridors, not only each pair's terminal band;
  outer nested pairs must retain space around inner pairs and intervening obstacles.
- [ ] Extend endpoint-only path extraction to real track-interior junctions. Exact
  contact connectivity already supports these; it does not establish path length.

## Domain planning and assessment

- [ ] Evaluate automatic multi-rail partitioning with layer/corridor, neck/current,
  fill and reference-plane checks; bounding-box pours are not sufficient.
- [ ] Evaluate whole-board Pareto feedback and connection-level complete-blocker
  ripping separately from implemented failing-tree branch checkpoints.
- [ ] Extend geometry-bound final-copper return/impedance/current checks and broader
  native acceptance cases. Routing closure never substitutes for CAM or hardware.

## Separate power/ground copper-area planning stage

Implementation sequence, subset trials and promotion gates are specified in the
[power area planning implementation plan](power-area-planning-plan.md). Start
with the consumer-owned single-domain baseline before expanding to the FX10 bank
or launching a new full-board comparison.

The opt-in standard pipeline now evaluates joint physical area proposals during
placement, adds bounded distribution/contact copper after package-boundary
reservation, and reports native continuity plus explicit-model DC mesh refinement.
Source/filter provenance, external return boundaries, exact ownership and local
associations remain separate contracts. Single- and two-domain CopperProbe
experiments, the USB power-bank partition and ADC supply with retained clock
copper pass scoped native continuity. Active bank/USB coexistence and full-board promotion remain
unfinished. See the plan for precise tested scope and remaining gates.

- [x] Add an explicit bounded planning stage that couples placement and routing
  through proposed power and ground areas. Run initial domain planning after
  coarse placement and before package access/detail routing, and feed infeasible
  pad access and corridor conflicts back to placement.
  Inputs include source rail/domain boundaries, stackup/reference layer roles,
  fixed poses/macros, pin-associated bypasses, fabrication constraints, current
  obligations, sensitive-node exclusions and critical signal corridor demand.
  Outputs include owned rail partitions, ground regions, short local supply and
  return connections/vias, reserved signal corridors and unresolved constraints.
  Use actual fresh native-filled fragments to validate continuity: source zone
  outlines and net names alone never establish a connection. Keep DC continuity,
  local high-frequency return-loop quality and engineering qualification separate.
  Preserve immutable geometry, required via multiplicity and verified exits;
  invalidate stale evidence and completely roll back rejected alternatives.
- [ ] Complete expanded consumer acceptance with active critical routes, all FX10
  domains and ADC/analog preservation before the normal full-board comparison.
  Test split islands, necks, antipads, signal reference discontinuities, competing
  rails, filtered downstream domains, unavailable tools and renamed geometries. Generic algorithms belong in
  CopperScript; board-specific supply policy and evidence remain in consumers.


The separate copper-area planner contract must include partial-zone fixtures:
a regulator/source outside a local supply island, disconnected same-net islands,
and a filtered downstream domain. Declaring a local zone must not silently make
outside terminals unroutable by deferring the entire net without a feed/contact
route mechanism. Require exact source-to-filled-fragment connectivity and local
bypass-loop obligations separately. Keep these fixtures and the bounded contact
contract distinct from implementing the entire planner.

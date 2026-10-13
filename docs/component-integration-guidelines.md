# Component integration guidelines for agents

Apply these rules when adding or replacing components in CopperScript examples,
projects or reusable circuit definitions, including components imported from
CopperLib. A correct pin map and electrical connectivity are only part of the
integration; preserve the manufacturer's layout-dependent requirements too.
This is contributor guidance, not a claim that the compiler enforces every item.

## Repository ownership: reusable components belong in library repositories

Generic component definitions and circuit modules that can be reused by other
projects belong in an appropriate library repository, together with their reusable
footprints, evidence, tests and physical hard macros. CopperLib is one instance;
parts may also come from other independently versioned library repositories.
For the LLM workflow improvement work, do not add components to CopperLib;
component additions in other library repositories are permitted. Check the
project's library dependencies before creating a duplicate; consuming projects
should import the shared package rather than copy
or maintain a project-local version. This applies even when the first use is a
single example or board.

Keep board-specific composition, placement, mechanics, selected operating
configuration and application constraints in the consuming project. Expose
reusable support/layout obligations through the library contract without baking
in one board's reference designators or coordinates. Clearly isolated synthetic
compiler test fixtures are not reusable component catalogue entries.

## RF components: always review matching and layout

1. Inspect the exact manufacturer's datasheet, reference schematic and reference
   layout for the selected part/package, bands and intended RF interface.
   Record source URLs, revision/page/figure locators and hashes for consumed
   assets. Do not treat distributor CAD as manufacturer layout qualification.
2. Explicitly determine whether chip matching, a balun, antenna tuning, filters
   or other RF support are required, already integrated, or unresolved. A
   nominal 50-ohm module port does not establish that its board antenna needs
   no matching. Record evidence when no external network is required; do not
   invent a network or omit one merely because ERC passes.
3. Check the reference layout for critical pin-to-part distances, orientations,
   topology, short interconnects, ground returns, layer/reference-plane and
   impedance requirements, return vias and antenna keepouts. Translate them
   into explicit physical constraints. Use a reviewed hard macro where fixed
   placement alone cannot preserve prescribed copper or matching-ground paths.
4. Check the placed and routed result with the real footprints and stackup.
   Component-centre proximity alone cannot prove short pin-to-pad routing.
   Verify transformed land positions and the backend's rotation convention;
   rotating a symmetric protection device can keep its courtyard unchanged
   while moving the signal land away from the connector. Source-fixed pin
   distances must be mutually feasible before copper-access optimization.
   For co-located transmitters/receivers, quantify maximum transmitted power
   including tolerance, minimum measured isolation, filter rejection and the
   receiver's immunity in its actual gain mode. The shared
   `pcbir.rf_coexistence.isolation_budget` API accepts integer mdB bounds and
   leaves unknown isolation/rejection incomplete. Point samples cannot be
   interpolated into band guarantees. Review filter passband loss, power/ESD,
   harmonics and non-antenna coupling independently; a favourable budget is
   not installed RF approval.
   Preserve antenna copper/component exclusions and avoid generic ground fills
   or stitching that bypass a prescribed matching return. Different packages,
   antennas, bands or stackups require reviewed adaptations and tuning; do not
   reuse values or dimensions blindly.

## Switched power: prefer a hard macro, otherwise strong constraints

For switching regulators, converters and switching chargers, ideally generate
a source-backed physical hard macro from the applicable manufacturer's reference
layout. Include topology-specific support and preserve input/output capacitors,
inductor placement, high-di/dt current/return loops, switching-node geometry,
quiet feedback/sense paths and thermal/ground connections. Bind exact footprints,
pad/net roles, copper, ports, keepouts and permitted transformations. Label
authored or adapted geometry honestly; it is not automatically vendor-qualified.

If a macro cannot be created or reused safely, at minimum:

- Declare strong, explicit relative/fixed placement, orientations and
  pin-relative distance limits for critical support components; soft placement
  groups and generic proximity preferences are not sufficient.
- Define trace widths or conductor geometry for critical power/current paths,
  based on operating peak/RMS current, copper thickness, temperature rise and
  voltage-drop limits. Review neckdowns at pads and via current capacity.
  Do not rely on default board widths or widen every net indiscriminately.
- Bound the actual critical routes and loop geometry, maintain short local
  returns and keep feedback/sense routing away from switching copper. A short
  distance between component origins does not establish a small switching loop.
- Encode requirements using supported physical constraints, routing profiles or
  macro assets, and test generated placement/routing against them. Explicitly
  document missing enforcement and require manual geometry review for those
  gaps; do not invent language syntax or silently relax limits to get a pass.

Numerical limits must come from the applicable reference or a documented
engineering derivation, not a universal guessed distance or trace width. If
the source does not establish a requirement, mark it unresolved and explain
what evidence is needed. A standalone reusable part may delegate host-specific
geometry, but must expose these support/layout obligations to its consumers.

## Verification and handoff

Keep evidence and unresolved adaptations with the part/circuit or board. Add
regression checks for matching topology, exact footprint/pad bindings, critical
placement and conductor dimensions where enforceable. Inspect all relevant
copper layers after native refill/DRC. Rebuild physical outputs when parts,
footprints, constraints or reference evidence change.

ERC, a legal placement, short routes and zero native DRC errors do not certify
RF performance, converter stability, power transients, thermal behavior or EMI.
State those qualification limits and do not describe an unresolved design as
production-ready.

Implementation references:

- [Physical hard macros](physical-hard-macros.md)
- [Language constraints and routing profiles](language-reference.md)
- [Device and part generation](device-generation.md)
- [CopperLib hard-macro review checklist](https://github.com/andenore/CopperLib/blob/main/docs/hard-macro-review-checklist.md)

## Via-free RF support trees

RF feeds default to point-to-point. A port net that also connects a shunt TVS
or tuning land may explicitly declare `topology = "tree"` with `max_vias = 0`.
The shared router then considers bounded surface trees joining every physical
land (including DNP and repeated-pad lands), retaining original layer, width,
clearance and total length budgets. Clock and differential profiles are not
silently reinterpreted. The local proposal is not a Steiner or RF network solver:
review branch lengths, launch discontinuities, component parasitics, ESD return
and reference geometry independently. A connected tree is not an RF approval.

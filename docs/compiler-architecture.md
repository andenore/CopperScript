# Compiler architecture

```text
.copper source
    -> lexer
    -> source-aware syntax tree
    -> hierarchical electrical IR
         |-> JSON IR
         |-> KiCad schematic backend
         `-> derived flat view -> electrical-rules checker
                              `-> power-state analyzer
                              `-> footprint resolver + draft physicalizer
                                      `-> physical IR
                                          |-> legal placement + routability estimate
                                          `-> KiCad PCB backend
```

The stages are intentionally separate so editor tooling and the [language
server](language-server.md) can reuse parsing, semantic lowering, and ERC independently.

Key modules:

- `pcbir.lsp` � versioned editor workspace, recovering source index, compiler-backed diagnostics and standard LSP stdio server.
- `pcbir.lexer` and `pcbir.parser` — dependency-free source frontend.
- `pcbir.syntax` — source locations and syntax-tree declarations.
- `pcbir.compiler` — semantic lowering and typed unit validation.
- `pcbir.packages` — manifests, package resolution, Git cache, and checksums.
- `pcbir.model` and `pcbir.quantities` — immutable semantic IR.
- `pcbir.elaborate` — explicit hierarchy-to-flat derivation for consumers that
  require a global connectivity view.
- `pcbir.erc` — reusable electrical-rules passes.
- `pcbir.power` — explicit steady-state power-domain analysis.
- `pcbir.physical` — immutable, backend-neutral board geometry, footprints,
  placements, nets, tracks, vias, stackup, and design rules.
- `pcbir.footprints` — deterministic `.kicad_mod` reference resolution from
  board-relative paths or explicit KiCad library roots.
- `pcbir.physicalize` — resolved-footprint and typed placement-constraint
  lowering; proxy geometry is an explicit development fallback.
- `pcbir.placement` — hierarchy-aware analytical placement, hybrid
  legalization, route-aware detailed refinement, and deterministic Pareto
  candidate generation.
- `pcbir.layout` — four-gate readiness orchestration and placement reporting.
- `pcbir.routing` — deterministic multilayer global routing with
  negotiated-congestion guides and explicit overflow diagnostics.
- `pcbir.routeflow` — full-route candidate selection, bounded legal placement
  moves, atomic rollback, stagnation control, and fresh-route certification.
- `pcbir.critical` — profile-driven locked copper for critical, differential,
  clock, CAN, RF-feed, power, and length/via-constrained routes.
- `pcbir.detailed` — guide-aware deterministic general routing with legal pad
  escape, exact foreign-copper clearance, locked critical copper, and bounded
  whole-pass rerouting. See [detailed-routing research](detailed-routing-research.md).
- `pcbir.geometry`, `pcbir.any_angle`, and `pcbir.shove` — shared exact integer
  predicates, deterministic visibility walkaround, and atomic recursive line
  shove with fail-safe rollback.
- `pcbir.drc` — fail-closed exact-copper checks, explicit coverage, scoped
  waivers, and content-bound physical signoff tokens.
- `pcbir.flow` — compatible global/critical/detailed routing and DRC
  orchestration with no proxy-to-copper shortcuts.
- `pcbir.manufacturing` — gated KiCad 10 Gerber/drill/netlist releases with
  independent structural CAM parsing, manifests, checksums, and atomic publish.
- `pcbir.process_drc` — separate provenance-bearing fabrication, stencil, and
  assembly capability gates.
- `pcbir.engineering` — evidence-graded screening results that explicitly state
  claim scope and model limitations.
- `pcbir.cam_qualification` and `pcbir.constraint_coverage` — fail-closed
  independent-tool evidence and end-to-end ownership of hard constraints.
- `pcbir.physical` — immutable placement, exact copper, typed zone/keepout
  intent, and content-bound external fill provenance. Manufacturing boards with
  zones are authoritatively refilled and saved by the pinned KiCad toolchain.
- `pcbir.importers.kicad_mod` — dependency-free, fail-safe KiCad footprint
  parser and normalizer.
- `pcbir.devicegen` — compact JSON/CSV device bundles, bounded extraction work
  packets, validation, and deterministic library generation.
- `pcbir.backends` — immutable backend artifacts and the KiCad schematic and
  PCB generators.
- `pcbir.serializer` — versioned JSON IR output.
- `pcbir.cli` — checking, compilation, KiCad export, and footprint inspection
  commands.

Python IR constructions are confined to test fixtures. `.copper` is the only
user-facing source format accepted by the compiler.

The [full-vertical acceptance design](full-vertical-example.md) combines
an STM32G0C1, CAN, an EG800G cellular modem over UART and USB, an nRF52832
Bluetooth slave, low-power motion sensing, GNSS, user I/O, and two SWD ports.
It is the integration fixture for progressing from CopperScript source toward
reviewed manufacturing outputs; its documentation distinguishes implemented
checks from the remaining production-layout work.

## Current ERC checks

- Unknown parts, components, pins, nets, and supply sources
- Duplicate component, net, supply, and interface names
- Pins assigned to multiple nets
- Multiple push-pull or power outputs on one net
- Supply voltages outside declared pin limits
- Invalid, missing, or off-net supply sources
- Disconnected and unsourced power inputs
- I²C signal bindings, pin compatibility, and SDA/SCL pull-ups
- MCU peripheral completeness, package-pin availability, and mux validity
- Duplicate exclusive peripheral/pin selections and incompatible mux-resource settings
- Package DNC/required-pin rules and required net traits
- Functional-unit endpoint resolution to canonical physical pins
- Differential-pair completeness and finite device-mode selections
- Parametric route-rule pad membership and power-domain operating ranges

`power-check` separately evaluates named rail states and warns when a driven net
may back-power an I/O domain declared off. It is intentionally a conservative
steady-state analysis, not firmware or transient simulation.

Physical constraints are separate from connectivity. During physical lowering,
placement regions, keepouts, fixed locations, legal orientations, alignment,
minimum/maximum distance, and explicit placement groups become typed physical
IR and are enforced by `plan-layout`. Hierarchy, interfaces, and proximity
constraints also produce soft semantic placement groups.

The physical Python API also supports [rigid reference-layout clusters](rigid-placement-clusters.md).
Unlike soft groups, these retain identity-bound local footprint poses and
keepouts during whole-cluster placement/refinement and routing feedback.
Full-vertical now binds the experimental TPS62130A and Nordic/Johanson
[hard macros](physical-hard-macros.md) by default. Their local copper is
locked, but regulator current/thermal performance and RF/antenna tuning remain
unqualified. See the [integration plan](full-vertical-macro-ground-plan.md).

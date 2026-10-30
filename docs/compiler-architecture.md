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

The implementation is the Rust workspace in `crates/`; the `copper` binary
wires the crates into commands. Key crates:

- `copper-lang` — lexer, recovering parser and source-aware syntax tree,
  package resolution (manifests, Git cache, checksums, locks), semantic
  lowering with typed unit validation (electrical, mechanical, silkscreen),
  hierarchy elaboration, ERC, power-state analysis, footprint references,
  the JSON IR serializer and the KiCad schematic backend.
- `copper-units` — exact decimals, integer nanometres, fractions, quantities
  and the shared `100 nF` formatter.
- `copper-json` — deterministic JSON parsing and output.
- `copper-state` — the `copperscript.state/v1` file format.
- `copper-physical` — exact board geometry (outlines, cutouts, holes, slots,
  line/arc paths), footprints and the dependency-free `.kicad_mod` importer.
- `copper-pcb` — the physical board IR (placements, nets, tracks, vias, zones,
  keepouts, stackup and rules), physicalization of resolved footprints (proxy
  geometry is an explicit development fallback), placement templates,
  hard-macro scenes and the KiCad PCB/project backend.
- `copper-place` — the placement planner behind `plan-layout`: seeds,
  legalization, rigid clusters, route-aware refinement, escape spacing,
  candidate selection and requirement verification.
- `copper-route` — global routing with negotiated-congestion guides, layer
  assignment, placement/routing feedback, escape planning, critical and
  differential-pair routing, the negotiated-congestion (PathFinder) unified
  router on a shared multilayer lattice with exact clearance, fanout, plane
  stitch, decoupling, native KiCad plane verification, the `route-board`
  pipeline and the `bench` ladder runner. See
  [detailed-routing research](detailed-routing-research.md).
- `copper-drc` — exact physical DRC with explicit coverage, copper
  connectivity, breakout spacing, placement legality, hard-macro
  materialization and signal-integrity screening (`si-check`).
- `copper-assembly` — assembly locks and checks, BOM/CPL and ordering CSVs,
  and native KiCad manufacturing files.
- `copper-report` — diagnostic explanations, a run's issues grouped by cause,
  board window views and engineering summaries.
- `copper-editor` — the routed editor overlay and the `edit-mechanical`
  browser editor (sessions, scene, reviewed source transactions, loopback
  HTTP server).
- `copper-lsp` — the language server behind `copper lsp`.
- `copper` — the binary and its command line.

`.copper` is the only user-facing source format accepted by the compiler.


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

The placement planner also supports [rigid reference-layout clusters](rigid-placement-clusters.md).
Unlike soft groups, these retain identity-bound local footprint poses and
keepouts during whole-cluster placement/refinement and routing feedback.
Full-vertical now binds the experimental TPS62130A and Nordic/Johanson
[hard macros](physical-hard-macros.md) by default. Their local copper is
locked, but regulator current/thermal performance and RF/antenna tuning remain
unqualified.

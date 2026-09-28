# CopperScript

CopperScript is a semantic, strongly typed language for describing PCB
connectivity, electrical intent, and design constraints.

The v0.1 compiler parses `.copper` source into a typed intermediate
representation and runs electrical-rules checks (ERC). It deliberately does not
claim fabrication readiness from routing guidance. Its prototype physical
workflow can place components and attempt geometry-checked detailed routing,
but a routed board still requires independent physical and KiCad DRC signoff.

## Quick start

Python 3.11 or newer is required. From the repository root:

```console
python -m copperscript check examples/valid_board.copper
python -m copperscript check examples/invalid_board.copper
python -m copperscript power-check examples/valid_board.copper
```

Lock all imported source and physical assets, then reproduce without network
access:

```console
python -m copperscript lock examples/full_vertical_board.copper --offline
python -m copperscript check examples/full_vertical_board.copper --locked --offline
python -m copperscript audit-footprints examples/full_vertical_board.copper --locked --offline --footprint-root path/to/kicad-footprints
```

Compile a valid design to normalized JSON IR:

```console
python -m copperscript compile examples/valid_board.copper -o board.json
```

Generate a self-contained KiCad 8 schematic:

```console
python -m copperscript export-kicad examples/valid_board.copper -o valid_board.kicad_sch
```

Generate a KiCad 8 PCB draft with resolved footprint geometry:

```console
python -m copperscript export-kicad-pcb examples/resolved_footprint_board.copper -o resolved.kicad_pcb
```

Direct `.kicad_mod` references are resolved relative to the board file. KiCad
`Library:Footprint` identifiers can be resolved through explicit roots:

```console
python -m copperscript export-kicad-pcb board.copper --footprint-root path/to/kicad-footprints
```

Placement remains a deterministic inspection grid and routing is not generated,
so the output is not yet fabrication-ready. Generated proxy pads remain
available for backend development through explicit opt-in:

```console
python -m copperscript export-kicad-pcb examples/valid_board.copper --allow-proxy-footprints
```

Produce a deterministic legal placement candidate and a four-gate readiness
report:

```console
python -m copperscript plan-layout examples/valid_board.copper --allow-proxy-footprints --candidates 3 -o planned.kicad_pcb --report layout-report.json
```

The planner estimates global routing congestion but does not generate copper.
Its report therefore marks Route as not run and Verify as blocked. See the
[physical layout workflow](docs/layout-workflow.md) for the research,
consolidated stages, algorithms, and limitations.
The [detailed-routing research](docs/detailed-routing-research.md) explains
the geometry checks, fabrication-rule profile, and current full-board limits.

Produce deterministic multilayer global-routing guides after transactional
placement feedback:

```console
python -m copperscript route-global examples/valid_board.copper --allow-proxy-footprints -o global-route.json
```

Global guides reserve corridors, layers, and proposed transitions; they are not
tracks or fabrication data.

Attempt detailed routing and write a native DRC report plus an inspection-only
KiCad PCB draft:

```console
python -m copperscript route-board examples/valid_board.copper --allow-proxy-footprints --report route-report.json -o routed-draft.kicad_pcb
```

The command exits nonzero when routing or DRC is incomplete. Even a successful
native check does not qualify proxy footprints or replace KiCad and CAM review.
`--pitch-mm`, `--passes`, and `--search-budget` bound detailed-routing work;
exhausting the search budget is reported per net rather than silently accepting
an unfinished path.
For difficult boards, `--fanout` pre-escapes crowded SMD pads, `--soft-ripup`
and `--maximum-ripup-blockers 4` try bounded transactional rerouting, and
`--detailed-feedback-trials 4` retries legal placement changes against exact
routing failures. These experiments do not waive physical DRC. Pass
`--verify-plane-fill "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"` to
refill a disposable KiCad copy and include authoritative open-net/island
findings in the report; a zone outline or stitched via alone is not proof of
connectivity. See [routing research tasks](docs/routing-research-tasks.md).

Validate and inspect a KiCad footprint before resolving it into a physical
design:

```console
python -m copperscript check-footprint path/to/package.kicad_mod --strict
```

For a development installation with the `copper` command:

```console
python -m pip install -e ".[test]"
copper check examples/valid_board.copper
python -m pytest
```

## Example

```copper
board SensorBoard {
    use library "tiny";
    import sensors "github.com/copperscript/examples/sensors";
    import stm32 "github.com/copperscript/examples/stm32";

    component U1: stm32.STM32G0B1CBT6;
    component U2: sensors.BME280;
    component R1: RESISTOR { value = 4.7kohm; }

    configure U1.I2C1 as SENSOR_BUS {
        SDA = PB7;
        SCL = PB6;
    }

    net I2C_SDA {
        U1.PB7;
        U2.SDA;
        R1.2;
    }

    supply V3V3 {
        voltage = 3.3V;
        external = true;
    }
}
```

See the [design specification](docs/design-specification.md), the
[language reference](docs/language-reference.md), and the complete [valid
example](examples/valid_board.copper).

Device-library authors should use the compact, table-driven
[device generation workflow](docs/device-generation.md) rather than writing
large MCU definitions by hand.

The implemented [rich device model](docs/rich-device-model-proposal.md) describes
how CopperScript can represent flexible Nordic pin routing, mode-dependent FX10
pins, differential ADC channels, multi-unit op-amps, and package connection
rules and the cross-vendor acceptance fixtures used to validate them.

The [hierarchical example](examples/hierarchical_board.copper) instantiates a
reusable [5 V to 3.3 V buck supply](examples/packages/power/buck_5v_to_3v3.copper)
and imports the sensor part from a separate package.

## Packages

Imports use stable URL-like package paths and an explicit local alias:

```copper
import power "github.com/copperscript/examples/power";
module PWR: power.Buck5VTo3V3;
```

Versions and development replacements are deliberately kept out of source
files. They live in `copper.mod`:

```text
module github.com/anden/CopperScript
require github.com/copperscript/examples v0.1.0
replace github.com/copperscript/examples => ./examples/packages
```

The compiler resolves the longest matching required module, loads all
`.copper` part and module definitions in the selected package directory, and
records downloaded content in `copper.sum`. Without a `replace`, v0.1 fetches
tagged GitHub modules into `.copper-cache`; package source is parsed as data and
no package code is executed. Local replacements remain editable and retain
their current content hash as IR provenance without being locked in the sum
file.

## Compiler architecture

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

The stages are intentionally separate so editor tooling and a future language
server can reuse parsing, semantic lowering, and ERC independently.

Key modules:

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
  whole-pass rerouting. See [detailed-routing research](docs/detailed-routing-research.md).
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

The [full-vertical acceptance design](docs/full-vertical-example.md) combines
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

## KiCad schematic backend

The initial backend targets KiCad 8's `20231120` `.kicad_sch` format. It embeds
deterministic generic symbols derived from part definitions, places symbols on
a backend-owned grid, and connects pins through short stubs and named labels.
Generated coordinates and UUIDs never enter the CopperScript IR.

This first revision emits one flat sheet. Hierarchical designs are elaborated
explicitly and retain their original qualified component paths in hidden
`CopperScriptPath` properties. Native KiCad hierarchical sheets and mappings to
standard KiCad symbols are planned follow-up work.

## Physical IR and KiCad PCB backend

The physical IR is separate from the electrical IR and stores exact geometry
as integer nanometres. It currently models polygonal board outlines, two-layer
stackups, basic design rules, footprint bodies, pads and courtyards, component
placement, regions, keepouts, fixed/relative rules, semantic groups, physical
net-to-pad assignments, track segments, and vias. It validates cross-references
when constructed.

The KiCad PCB backend targets KiCad 8's `20240108` board format. It emits a
self-contained board with deterministic UUIDs, embedded footprints, net
assignments, copper tracks and vias when present, and a closed `Edge.Cuts`
outline. Each PCB export also writes a same-stem `.kicad_pro` with the physical
IR's minimum clearance and default net-class widths/vias; open the board with
that project for KiCad DRC. Backend tests include a completely routed synthetic
physical board. Generated reference designators are on fabrication layers
until a collision-aware silkscreen labeling pass is available.

The `.copper` frontend supports selected placement, routing, and copper-zone
constraints, while arbitrary board-outline authoring is still pending.
`export-kicad-pcb` resolves selected `.kicad_mod` files into
physical IR, checks that their numbered pads exactly match the electrical part,
and arranges components on a deterministic grid. These unrouted drafts must not
be sent for fabrication. Generic proxy geometry is available only with
`--allow-proxy-footprints` and remains clearly marked in output warnings.

`plan-layout` replaces the inspection grid with a deterministic, clearance-legal
placement and emits coarse routability metrics. It still creates no tracks and
explicitly blocks fabrication sign-off.

## KiCad footprint importer

`load_kicad_mod()` and `parse_kicad_mod()` convert KiCad 6–8 footprint files
into `PhysicalFootprint`. The initial importer covers common SMD, plated and
non-plated through-hole pads, circular and oval drills, rotations, independent
mask/paste presence, round-rectangle ratios, and the usual line, rectangle,
circle, arc, and polygon graphics. Imported geometry can be emitted directly by
the KiCad PCB backend.

The importer is intentionally fail-safe. Custom/trapezoid pads, copper or mask
graphics, drill offsets, and unrepresented fabrication modifiers are rejected.
Presentation-only omissions such as 3D models and user text produce explicit
warnings; `--strict` promotes those warnings to errors. Source format, version,
generator, path, and SHA-256 checksum are retained as footprint metadata.

Footprint references ending in `.kicad_mod` are paths relative to the board
source. `Library:Footprint` searches `<root>/Library.pretty/Footprint.kicad_mod`
and `<root>/Library/Footprint.kicad_mod` for each `--footprint-root`. Resolution
fails on zero or multiple matches, and the selected name must agree with the
footprint name declared inside the file. CopperScript never searches an
installed KiCad library implicitly.

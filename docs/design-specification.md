# CopperScript design specification

**Status:** Draft specification with accepted architectural decisions
**Applies to:** CopperScript language, compiler, IRs, rule engines, and backends
**Last updated:** 2026-09-20

This document records the durable design decisions for CopperScript. It is the
normative architectural reference; the README explains usage and the language
reference describes current syntax.

The terms **MUST**, **MUST NOT**, **SHOULD**, and **MAY** express requirement
strength. Sections marked "Current implementation" describe temporary reality
and do not override the target architecture.

## 1. Purpose

CopperScript is a semantic language for electronic design. Authors describe
components, connectivity, electrical intent, and constraints. Tooling derives
validation results, exchange artifacts, and eventually physical layouts.

The central principle is:

> Describe electrical intent and constraints explicitly. Describe geometry only
> where physical geometry is itself the intent.

CopperScript is not a textual encoding of a schematic drawing application and
must not become one.

## 2. Architectural domains

The compiler architecture separates four domains:

```text
CopperScript source
        |
        v
+----------------------+       +----------------------+
| Electrical design IR |       | Constraint set       |
| parts, instances,    |<------| rules and targets    |
| nets, supplies,      | refs  | no connectivity      |
| interfaces           |       | ownership            |
+----------------------+       +----------------------+
        |                               |
        +---------------+---------------+
                        v
                +------------------+
                | Layout engine    |
                +------------------+
                        |
                        v
                +------------------+
                | Physical IR      |
                | placement,       |
                | routing, stackup |
                +------------------+
```

Schematic presentation is not another authoritative IR domain. It is a backend
artifact derived from the electrical design.

### 2.1 Electrical design IR

The electrical IR defines what the circuit is. It MAY contain:

- reusable part and pin definitions;
- package-independent device pads, electrical profiles, functional units,
  signal groups, power domains, peripherals, and route rules;
- component instances and selected footprints;
- explicit peripheral and pin-mux selections;
- nets and their endpoints;
- supplies, nominal voltages, and power sources;
- named discrete power states for analysis;
- protocol interfaces and their signal bindings; and
- typed electrical properties required for validation or generation.

It MUST NOT contain:

- schematic symbol coordinates or rotations;
- wire paths, junction drawing positions, or label positions;
- pages, sheets used only for drawing organization, title blocks, or view state;
- colors, fonts, zoom, selection, or other UI state;
- PCB placement, routing, board-outline, stackup, zone, or via geometry; or
- backend-specific object identifiers when they can be derived deterministically.

### 2.2 Constraint set

Constraints express requirements on a design; they do not define connectivity.
The compiler MUST lower constraints into a logically separate `ConstraintSet`
that refers to electrical objects through stable semantic references.

Changing or removing a constraint MUST NOT silently add, remove, or reconnect a
net endpoint. Conversely, moving a component between nets MUST require an
explicit connectivity edit rather than a constraint change.

Constraints MAY cover multiple domains, including:

- electrical limits, such as voltage, current, impedance, or timing;
- placement requirements, such as maximum distance or keep-together groups;
- routing requirements, such as width, clearance, topology, or length matching;
- manufacturing requirements; and
- advisory notes that remain machine-addressable.

Constraints SHOULD be declarative and typed. A constraint states the required
outcome, not a sequence of operations for producing it.

Source syntax MAY place constraints near related declarations for readability,
but source proximity MUST NOT merge their semantic ownership with connectivity.

### 2.3 Physical layout IR

The physical IR will describe one concrete realization of an electrical design.
It MAY contain board outline, stackup, component placement, footprint geometry,
tracks, vias, copper zones, and other fabrication geometry.

The physical IR MUST be a separate model generated from:

1. the electrical design IR;
2. the constraint set;
3. selected library and fabrication data; and
4. layout-engine decisions.

PCB coordinates belong here. Schematic drawing coordinates do not.

The physical IR MUST retain traceable references to the electrical objects it
implements. A track, pad, or zone may identify a net; it must not redefine that
net's logical membership.

The initial physical IR uses integer nanometres for coordinates and dimensions
so backend output is deterministic and does not accumulate binary floating
point error. It contains a closed polygonal board outline, stackup, basic design
rules, resolved footprint geometry, placements, physical net-to-pad mappings,
track segments, and vias. Physical objects use stable electrical component and
net names for traceability.

The model validates its internal references independently of any backend. A
placement must resolve a footprint, a net endpoint must resolve an existing
placement and pad, and copper geometry must resolve a physical net and stackup
layer. This structural validation is not a substitute for geometric DRC.

### 2.4 Presentation and tool artifacts

Backends MAY generate coordinates and other presentation data required by a
target format. Such data is backend-owned and MUST NOT flow back into the
electrical IR merely because a target application requires it.

For example, a KiCad schematic backend may arrange symbols on a grid and place
net labels. Those positions are rendering decisions, not CopperScript design
semantics.

## 3. Core semantic decisions

### 3.1 Connectivity has one source of truth

`Net.endpoints` is the canonical connectivity definition. Components, supplies,
interfaces, constraints, and backend renderings MUST NOT create a second
connectivity mechanism.

The compiler MUST reject or diagnose a pin assigned to more than one net.

### 3.2 Parts and component instances are distinct

A `PartDefinition` describes reusable facts about a part: pins, electrical pin
capabilities, device-pad bonds, limits, manufacturer data, and compatible
physical packages.

A `ComponentInstance` describes use of that part in one design: reference,
value, chosen footprint, and instance properties.

Backend symbol bindings and library lookup information SHOULD live in explicit
adapter or library metadata rather than in user-authored connectivity.

### 3.3 Supplies are explicit semantic objects

A supply annotates an existing net with voltage and sourcing information. It
does not create the net or connect pins implicitly.

Power-source validation and voltage-range checking consume supply annotations.
Ground is not a special parser primitive; it is a named supply/net with explicit
semantics.

### 3.4 Interfaces overlay connectivity

Interfaces describe protocol-level meaning, such as I²C signal roles and device
bindings. They refer to existing nets and pins and MUST NOT duplicate or replace
net membership.

Protocol-aware checks MAY require supporting components, such as I²C pull-up
resistors, by examining ordinary components and nets.

### 3.5 Quantities are typed

Physical values MUST carry dimensions. Units are normalized internally while a
display unit may be retained for diagnostics and serialization.

Cross-dimension operations, such as comparing voltage with resistance, MUST be
rejected. Backends MUST NOT infer a unit from an untyped number where the
language requires a physical quantity.

### 3.6 References are semantic, not positional

Components, nets, interfaces, and constraints refer to stable semantic names or
IDs. Source offsets and generated coordinates are provenance or presentation;
they are not design identity.

Renaming support should eventually update all semantic references through the
compiler model rather than textual search and replace.

### 3.7 Hierarchy is reusable electrical structure

A module defines a reusable electrical design with typed boundary ports. A port
MUST connect to exactly one internal net, and a parent connects a module instance
through ordinary net endpoints. Ports therefore expose connectivity without
creating a second connectivity system.

Modules MAY instantiate other modules. Circular imports or instantiation cycles
MUST be rejected.

The authoritative electrical IR MUST preserve module definitions, module
instances, typed ports, local component and net names, and boundary connections.
A module definition MUST be represented once regardless of how many times it is
instantiated. Compilation MUST NOT replace this structure with a flattened
netlist.

Hierarchy elaboration is an explicit derived operation. It MAY produce
deterministic qualified identities such as `PWR/U1` for ERC, global analysis, or
backends that need a flat connectivity view. The derived view MUST NOT become
the source of truth and MUST NOT be required by hierarchy-aware backends.

Constraints declared inside a module are instantiated and qualified with that
module. They remain in the constraint domain rather than becoming part of the
module's connectivity definition.

### 3.8 Packages have stable source identities

Reusable parts and modules are shared through packages identified by URL-like
paths such as `github.com/vendor/library/sensors`. Source files import a package
under a local alias and refer to its exports through qualified names. An import
path identifies source; it does not select a version.

Dependency versions and local development replacements MUST live in the
project-level `copper.mod`, not in individual source imports. Downloaded module
content MUST be authenticated by `copper.sum`. All resolved dependencies,
including mutable local replacements, retain their current content hash as
compiled IR provenance. A package is declarative: the compiler parses `.copper`
definitions and MUST NOT execute dependency-provided code.

Resolution uses the longest required module-path prefix. v0.1 supports GitHub
Git repositories and explicit local `replace` directives; this may later be
generalized without changing source import identity.

Reusable production part, device, footprint, and circuit-module definitions
MUST be maintained in the separate CopperLib project, whose stable module path
is `github.com/andenore/CopperLib`. The CopperScript compiler repository MAY
contain deliberately tiny built-in fixtures and compiler tests, but product
examples MUST consume newly created reusable definitions from CopperLib rather
than duplicate them locally. Local development uses an explicit `copper.mod`
replacement for the sibling CopperLib checkout.

### 3.9 Complex devices separate capabilities from packages

Complex programmable parts such as MCUs MUST distinguish three concepts:

- a package-independent `DeviceDefinition` describing named silicon pads,
  power domains, peripherals, peripheral signals, mux options, and shared
  configuration resources;
- a `PartDefinition` describing one orderable/package variant, its physical
  package pins, and explicit bonds from those pins to device pads; and
- a `PeripheralSelection` describing the explicit mux choices made for one
  component instance.

A device pad and a physical package pin are different identities. A mux option
maps a device pad to one peripheral signal and retains the target-specific
selector needed by a backend, such as `AF6`. A package pin may bond to one or
more device pads, and multiple package pins may bond to the same logical pad
where the package requires it. ERC resolves a selected package pin through its
declared bonds before validating mux availability.

Physical pins and device pads use an `ElectricalProfile` rather than one pin
type or capability bag. Signal domains, directions, drive modes, extensible
traits, and typed ranges are orthogonal. A pad may therefore be analog and
digital, bidirectional, and support both push-pull and open-drain drive without
inventing a combined enum value. Protocol signals declare required profiles;
ERC checks that a selected physical endpoint satisfies them.

Part categories and interface type names are open strings. Standard meanings
use the `std.` namespace, while dependency packages may define namespaced
vocabulary. Generic ERC MUST consume typed fields rather than infer behavior
from an unknown taxonomy string.

Functional units are package-independent semantic views over device pads. A
unit terminal resolves through its pad and package bonds to one canonical
physical pin. Source endpoints MAY use paths such as `U1.A.OUT`; connectivity
and pin-on-multiple-net checks operate on the resolved physical identity. Units
do not create connectivity and may guide backend presentation without adding
schematic geometry to IR.

Signal groups describe relationships such as positive and negative members of
a differential pair. Package pins carry explicit normal, required,
do-not-connect, or optional connection policy and may require net traits such
as `ground`. These are structural electrical rules, not name-based heuristics.

Finite mode groups and equality-only conditions control pad, bond, group, or
route availability. A component makes an explicit choice or uses a declared
default. General boolean expressions and conditions derived from connectivity
are not part of the language.

Explicit mux rows remain appropriate for irregular mappings such as STM32
alternate functions. Regular routing such as Nordic PSEL uses concrete named
pad sets and a typed selector scheme, avoiding an N-by-M expansion while still
retaining a deterministic concrete selection in compiled IR.

Optional resource and setting fields express device-wide configuration state.
Selections are exclusive by default. Exclusive selections requiring different
settings for the same resource conflict; multiple signals requiring the same
setting are compatible. A selection explicitly marked `firmware_managed` MAY
share a peripheral, pin, or resource only when every conflicting selection is
also firmware-managed. This
waiver suppresses only resource-ownership conflicts: unknown pads, invalid mux
options, missing required signals, and electrical incompatibilities remain
errors. A design may omit peripheral configuration and use a pin as ordinary
GPIO when detailed firmware mode checking is not useful.

Peripheral configuration annotates component behavior and MUST NOT create or
modify net connectivity. A configured pin is connected only by appearing as a
net endpoint. This preserves nets as the single source of electrical
connectivity.

v0.1 requires explicit pin assignments. Automatic pin assignment MAY later be
implemented as constraint solving, but its result MUST lower to the same
concrete `PeripheralSelection` IR and retain every selected pin and mux
selector. Device packages are declarative data. Source document, revision,
location, URL, and checksum are optional metadata and MUST NOT be required for
compilation. Generated production libraries SHOULD retain such provenance when
available so data can be audited; hand-authored or experimental definitions may
omit it.

### 3.10 Power domains and discrete power states

Device pads MAY belong to named power domains. Each domain identifies the
device supply pads that establish its powered state. I/O pads may also declare
their unpowered behavior as `high_impedance`, `tolerant`, `clamped`, or
`unknown`.

A board MAY define named power states by assigning each relevant `Supply` the
state `on`, `off`, or `unknown`. Power states annotate the existing electrical
design; they MUST NOT create rails, sources, switching connections, or implicit
net membership. Modules do not own board-level power scenarios in v0.1.

Power-state analysis is a separate IR consumer from ordinary ERC. The v0.1
analyzer performs conservative steady-state checks, including warning when a
driven net reaches a clamped or unknown I/O pad whose domain is off. It does not
simulate firmware, analog transients, ramp timing, regulator dynamics, or the
internal behavior of power switches. Later sequencing and transition models
MUST build on the same explicit domains and states rather than embedding a
hidden simulator in connectivity checking.

### 3.11 Generated device libraries use compact normalized data

Large devices SHOULD be maintained as normalized metadata and tables rather
than hand-authored repetitive CopperScript. The repository generation workflow
uses JSON for device/package metadata and CSV for pads, pins, peripheral
signals, and mux options. Deterministic tooling validates references and emits
ordinary `.copper` definitions; generated source remains declarative and is
compiled through the same frontend as hand-written packages.

The normalized bundle is authoritative library input. Generated CopperScript
MUST be reproducible and SHOULD carry a generated-file notice. CI SHOULD fail
when checked-in generated files differ from the bundle.

Unknown or disputed extracted values use an explicit marker and MUST block
generation. Tooling SHOULD produce small work packets containing only selected
or unresolved rows plus the relevant enum vocabulary and optional provenance.
This permits datasheet agents to work on bounded tables without repeatedly
loading complete devices or emitting repetitive syntax.

Machine-readable vendor data SHOULD be preferred over PDF extraction. Source
provenance remains optional as required by CS-027, but generators SHOULD retain
it whenever available. Generation tooling MUST NOT execute dependency-provided
code.

## 4. Language and compiler

`.copper` is the only user-facing source format. Executable Python is permitted
only in internal tests and development fixtures.

The compiler pipeline is:

```text
UTF-8 source
    -> lexer
    -> source-aware syntax tree
    -> semantic lowering
    -> electrical IR + constraint set
    -> ERC and constraint validation
    -> one or more backend artifacts
```

Each stage SHOULD have a narrow API and be independently testable. Parsing MUST
NOT perform backend generation. Backends MUST NOT reparse CopperScript source.

Diagnostics MUST have stable machine-readable codes. Syntax and semantic
diagnostics SHOULD include source spans. ERC diagnostics must eventually map
back to the declaration or reference that caused them so a language server can
publish precise editor diagnostics.

Compilation MUST be deterministic for identical source, libraries, compiler
version, and options.

## 5. Validation

Electrical-rules checking is a consumer of the electrical IR, not parser logic.
It SHOULD remain usable by tests, editor tooling, importers, and backends.

ERC currently covers unknown references, duplicate references, pins on multiple
nets, conflicting outputs, supply voltage limits, unsourced power inputs, basic
I²C rules, and explicit device mux selections. Device checks include package-pin
availability, required peripheral signals, valid mux choices, exclusive pin and
peripheral use, electrical compatibility, and shared resource settings. New
checks SHOULD be independent passes with stable diagnostic codes.

Discrete power-state analysis is also an independent IR pass. It reports
unknown state rails and possible back-power paths without changing electrical
validity or connectivity. Firmware-managed sharing waives ownership conflicts,
not structural or electrical validation.

Constraint validation is distinct from ERC even when both run under `copper
check`. This separation allows electrical validity, constraint consistency, and
physical feasibility to be reported independently.

A backend MUST NOT silently repair an invalid electrical design. Generation may
stop on errors or proceed only through an explicit diagnostic/testing option.

## 6. Backend architecture

A backend consumes compiled semantic models and produces named artifacts. It
MUST NOT mutate the electrical IR or constraint set.

Conceptually:

```text
Backend.generate(
    electrical_design,
    constraint_set,
    backend_options,
) -> ArtifactManifest
```

An artifact manifest may contain one or more files, warnings, target-version
metadata, and provenance.

Target-specific names, symbol mappings, file versions, and generated UUIDs
belong in the backend or its adapter registry. They SHOULD NOT leak into the
core electrical model unless they represent genuine cross-backend semantics.

Backends SHOULD generate stable ordering and deterministic identifiers so small
source changes produce reviewable diffs.

### 6.1 KiCad schematic backend

The first external backend generates a KiCad schematic as an
interoperability artifact. Its goals are to:

- validate that CopperScript electrical semantics map to a real EDA tool;
- provide a visual, inspectable schematic;
- carry references, values, footprints, nets, and labels into KiCad; and
- support KiCad's normal schematic-to-PCB workflow.

It is not a layout engine and MUST NOT introduce schematic coordinates into the
electrical IR.

The initial implementation:

- targets KiCad 8 and schematic format `20231120`;
- creates embedded generic symbols from CopperScript part definitions, with a
  future adapter layer responsible for mappings to standard KiCad libraries;
- creates deterministic, version-4-shaped UUIDs from stable semantic identity;
- uses a simple backend-local grid arrangement;
- uses short pin stubs and named net labels instead of aesthetic wire routing;
  and
- verifies generated artifacts through deterministic fixtures and should also
  use KiCad tooling when it is available.

The first revision emits one flat sheet by explicitly elaborating the compiled
hierarchy. Qualified CopperScript instance paths are retained in hidden
`CopperScriptPath` properties when they cannot be used directly as KiCad
references. This is an interoperability limitation, not a change to the
authoritative hierarchical IR. Native KiCad sheets are planned after the basic
artifact path is proven.

The generated schematic is not the source of truth. Round-trip import and
back-annotation are outside the first implementation and require a separate
design decision.

### 6.2 KiCad PCB backend

A KiCad PCB backend consumes the physical IR, not schematic drawing data. It
serializes placement and routing decisions while preserving references to
electrical nets and components.

Generating an unrouted board skeleton directly from the electrical IR MAY be a
useful intermediate feature, but any automatically chosen placement belongs to
backend output or physical IR, never to the electrical IR.

The initial implementation:

- targets KiCad 8 board format `20240108`;
- emits embedded footprint pads and simple body outlines rather than relying on
  the receiving installation's footprint libraries;
- emits physical net assignments, track segments, vias, and a closed
  `Edge.Cuts` outline;
- uses deterministic, version-4-shaped UUIDs derived from stable object
  identity; and
- accepts only `PhysicalBoard`, making the electrical/physical boundary
  explicit in its public API.

The temporary physicalization adapters are not placement engines. The normal
path resolves verified footprints and creates deterministic grid placement;
`prototype_physicalize` additionally creates generic proxy pads only when the
caller explicitly requests that development fallback. Both outputs MUST be
marked as not fabrication-ready, and the backend MUST report the remaining
placement/routing limitations as warnings.

### 6.3 Physical-design workflow

The user-visible physical-design workflow has exactly four gates: Prepare,
Place, Route, and Verify. Detailed activities such as floorplanning, global
placement, legalization, local refinement, and routability estimation are
internal Place substeps rather than independent public phases. Global corridor
and layer assignment, detailed routing, rip-up/reroute, and tuning are Route
substeps. DRC, electrical/thermal/RF analysis, DFX, and CAM inspection are
independently reportable Verify checks.

A completed Place gate MUST mean that all represented components are inside the
board and satisfy represented component-clearance constraints. It SHOULD include
a coarse global-routing estimate so congestion can feed back into placement.
Such an estimate MUST NOT be represented as routed copper. Route remains not run
until geometrically valid tracks and vias exist, and Verify MUST remain blocked
until required sign-off checks have passed.

Placement MUST consume typed physical constraints rather than interpreting
source text. The physical IR represents courtyard geometry, keepouts, placement
regions, fixed placement, legal orientations, minimum/maximum distance,
alignment, and semantic groups. Groups derived from preserved module hierarchy,
interfaces, and proximity constraints provide soft clustering intent without
changing electrical connectivity.

The initial placement engine uses deterministic multi-seed analytical global
placement, hybrid discrete legalization, hard relative-rule repair, and legal
local refinement with coarse per-layer routing feedback. Wirelength and
routability metrics use transformed footprint pads. Multiple legal results are
Pareto-filtered and deterministically ranked; reports MUST retain both the
selected result and candidate metrics. Proxy crossings, vias, pin escape, and
congestion are estimates only and MUST NOT be serialized as routed copper.

Automated placement and routing artifacts MUST retain their algorithm identity,
gate state, metrics, and non-fabrication-ready status. See
`docs/layout-workflow.md` for the researched algorithm choices and implemented
scope.

### 6.4 Footprint importers

External footprint formats are adapters into `PhysicalFootprint`; they are not
alternate physical IRs. Importers MUST normalize exact geometry, retain source
identity and checksum metadata, and report every unsupported construct. They
MUST reject fabrication-relevant geometry that cannot be represented rather
than silently approximating or dropping it.

The KiCad `.kicad_mod` importer parses KiCad 6–10 S-expressions without
executing code or requiring KiCad. It supports ordinary SMD and through-hole
pads, circular and slotted drills, pad rotations, mask and paste layer presence,
paste-only apertures, per-pad zone/heatsink/layer-removal properties,
footprint-local clearance, round-rectangle ratios, and common footprint drawing
primitives including mask and paste artwork. Simple footprint-local copper
keepouts are typed, transformed with each placement (including back-side
mirroring), conservatively excluded from routing grids, checked against exact
copper geometry in DRC, signed with the geometry, and emitted
as deterministic KiCad board zones. Active footprint-placement keepouts,
connected footprint zones, and polygon holes remain unsupported and fail
closed. Ignored
presentation-only constructs produce warnings, and strict mode promotes all
warnings to errors. Custom pads, copper graphics, drill offsets, and
unsupported fabrication modifiers are errors until the physical IR can retain
them losslessly.

Footprint selection and footprint resolution are separate phases. The
electrical IR retains the user-selected string; physical lowering resolves it
through a dedicated adapter. A direct `.kicad_mod` path is relative to the
board source. A KiCad `Library:Footprint` identifier is searched only in
explicit caller-provided roots. The resolver MUST NOT inspect an installed EDA
tool's implicit library configuration because that would make builds
machine-dependent. No match and multiple matches are both errors.

The selected name MUST agree with the name declared inside the footprint file.
Before placement, the set of non-empty electrical pad numbers MUST exactly
match the part's physical pin numbers. Duplicate footprint primitives sharing
one pad number are allowed, and unnumbered non-plated mounting holes are ignored
for electrical matching. Proxy geometry requires explicit opt-in and MUST
remain marked as non-fabrication-ready.

## 7. Serialization and versioning

Serialized IR MUST declare a schema identifier and version. Readers MUST reject
unsupported incompatible schema versions rather than guessing.

The default serialized electrical IR MUST preserve hierarchy. A serialized flat
view, if offered for debugging or interoperability, MUST be clearly identified
as derived and non-authoritative.

Serialization SHOULD be stable and suitable for testing, inspection, and tool
integration. It is a compiler artifact, not necessarily the long-term source
format.

Backend artifacts MUST identify CopperScript as their generator where the
target format provides such a field.

## 8. Non-goals for v0.1

The following are deliberately outside the current scope:

- production-ready automatic PCB placement and routing;
- copper pours and direct fabrication output;
- schematic beautification beyond a deterministic basic rendering;
- importing arbitrary KiCad projects into CopperScript;
- lossless round trips through external EDA tools;
- advanced design variants and parameterized/generic modules; and
- global component sourcing or lifecycle management.

These exclusions prevent target-tool details from distorting the language
before its semantic core is stable.

## 9. Current implementation and required refactoring

The current `Board` dataclass combines electrical objects and a `constraints`
tuple. This is transitional. Before physical-layout work expands, it SHOULD be
replaced by a package-level result similar to:

```text
CompiledDesign
  electrical: ElectricalDesign
  constraints: ConstraintSet
```

The current constraint syntax may remain unchanged; the separation is semantic
and architectural, not necessarily visual in source.

Footprint strings are opaque selections in electrical IR. The physical
resolver interprets explicit `.kicad_mod` paths and KiCad library identifiers;
the KiCad PCB backend itself receives only normalized physical IR.

## 10. Open design questions

These questions are intentionally unresolved:

- Should stable identities be explicit in source or derived from qualified
  semantic paths?
- Should stable hierarchical identities remain path-derived if instances are
  renamed, or should source-level persistent IDs be introduced?
- Can separate constraint-profile files override or extend inline constraints?
- What is the serialized schema for the physical IR?
- Which external-tool edits, if any, are safe to back-annotate?
- How are multiple footprint candidates selected before layout?

An open question MUST NOT be treated as an implicit decision by a backend.

## 11. Decision log

| ID | Status | Decision |
|---|---|---|
| CS-001 | Accepted | CopperScript describes semantic intent, not drawing primitives. |
| CS-002 | Accepted | `.copper` is the sole user-facing source format. |
| CS-003 | Accepted | Net endpoint lists are the single source of connectivity. |
| CS-004 | Accepted | Part definitions and component instances are separate concepts. |
| CS-005 | Accepted | Physical quantities are dimensioned and unit-aware. |
| CS-006 | Accepted | Supplies and interfaces annotate existing connectivity. |
| CS-007 | Accepted | Constraints are semantically separate from electrical connectivity. |
| CS-008 | Accepted | Electrical IR contains no schematic UI or presentation coordinates. |
| CS-009 | Accepted | PCB geometry belongs in a separate physical IR. |
| CS-010 | Accepted | Backends are target-specific consumers of compiled models. |
| CS-011 | Accepted | Backend-generated identifiers and output are deterministic. |
| CS-012 | Accepted | KiCad schematic generation is the first interoperability backend. |
| CS-013 | Accepted | Generated schematics are artifacts, not authoritative design input. |
| CS-014 | Accepted | ERC remains independent of parsing and backend generation. |
| CS-015 | Accepted | Serialized IR is explicitly schema-versioned. |
| CS-016 | Accepted | Modules expose typed ports through ordinary net endpoints. |
| CS-017 | Accepted | The authoritative IR preserves hierarchy; flattening is an explicit, non-authoritative derived view. |
| CS-018 | Accepted | Reusable parts and modules are imported from URL-like package paths under explicit local aliases. |
| CS-019 | Accepted | Dependency versions and local replacements belong in `copper.mod`, not source imports. |
| CS-020 | Accepted | Downloaded package content is verified by `copper.sum`; all dependencies retain hash provenance and dependency code is never executed. |
| CS-021 | Accepted | Package-independent device capabilities are distinct from package-specific physical part pins. |
| CS-022 | Accepted | Peripheral configuration annotates pin behavior and never creates connectivity; nets remain authoritative. |
| CS-023 | Accepted | Explicit mux selections lower to typed IR containing concrete pins, selectors, and shared resource settings. |
| CS-024 | Accepted | Automatic pin assignment is deferred and must eventually lower to the same explicit selection IR. |
| CS-025 | Accepted | Device pads and physical package pins are distinct identities connected by explicit bonds; mux options name device pads. |
| CS-026 | Accepted | Physical pins and device pads carry capability sets rather than one mutually exclusive electrical pin type. |
| CS-027 | Accepted | Source-document and revision provenance is optional; production generators should retain it when available. |
| CS-028 | Accepted | Peripheral selections are exclusive by default; `firmware_managed` suppresses ownership/resource-sharing conflicts but not structural or electrical validation. |
| CS-029 | Accepted | Power domains and named discrete rail states are explicit IR, analyzed by a separate conservative steady-state pass. |
| CS-030 | Accepted | The initial schematic backend targets KiCad 8 format `20231120`, owns all presentation geometry, embeds generic symbols, and explicitly derives a flat single-sheet artifact. |
| CS-031 | Accepted | Large device libraries use compact normalized JSON/CSV bundles and deterministic source generation; unresolved values block generation and bounded work packets minimize extraction context. |
| CS-032 | Accepted | Electrical profiles separate signal domains, directions, drive modes, traits, and typed ranges. |
| CS-033 | Accepted | Part categories and interface type names are open; generic behavior depends on typed semantics, not taxonomy strings. |
| CS-034 | Accepted | Functional-unit terminals resolve to canonical physical pins and never create connectivity. |
| CS-035 | Accepted | Signal groups represent differential and other multi-signal relationships independently of protocols. |
| CS-036 | Accepted | Package pins have typed connection policies, required net traits, and conditional bonds. |
| CS-037 | Accepted | Device modes are finite named choices and conditions are conjunctions of equality selections. |
| CS-038 | Accepted | Regular pin routing uses concrete pad sets and selector schemes; irregular mappings retain explicit mux options. |
| CS-039 | Accepted | The physical IR is a backend-neutral immutable model using integer nanometres, validated references, and traceability to electrical component and net identities. |
| CS-040 | Accepted | The KiCad PCB backend consumes only physical IR and initially targets KiCad 8 format `20240108`; prototype proxy footprints are explicitly non-fabrication-ready. |
| CS-041 | Accepted | Footprint importers normalize external files into physical IR, retain checksum provenance, and fail instead of silently losing unsupported fabrication geometry. |
| CS-042 | Accepted | Footprint resolution is explicit and deterministic: board-relative files or caller-provided library roots only; ambiguity and electrical pad mismatches are errors. |
| CS-043 | Accepted | CopperLib is the canonical project for new reusable part, device, footprint, and circuit-module definitions; product examples import them rather than duplicating them in the compiler repository. |
| CS-044 | Accepted | Physical design exposes four gates—Prepare, Place, Route, Verify. Placement includes legalization and routability feedback; estimates never masquerade as routed copper, and release remains blocked until routing and sign-off pass. |
| CS-045 | Accepted | Placement consumes typed physical constraints and preserved semantic hierarchy, then uses deterministic analytical global placement, hybrid legalization, pad-aware route feedback, detailed refinement, and Pareto-ranked candidates. |
| CS-046 | Accepted | Global routing uses deterministic multilayer capacity guides and negotiated congestion; guides and proposed vias are planning artifacts and only detailed routing may create physical copper. |
| CS-047 | Accepted | Placement–routing feedback is transactional: only complete global-route improvements are accepted, rejected moves roll back atomically, and a fresh full reroute is required for certification. |
| CS-048 | Accepted | Critical nets route before ordinary nets from explicit physical profiles; coupled bundles and exact locked copper retain external qualification assumptions, and geometric proxies never claim impedance, SI, RF, current, or thermal signoff. |
| CS-049 | Accepted | General detailed routing consumes global guides, preserves locked critical copper, owns exact tracks and vias, and uses deterministic negotiated rip-up/reroute; routing success never substitutes for physical DRC. |
| CS-050 | Accepted | Physical signoff is fail-closed and reports check coverage separately from findings; exact waivers are fingerprint-bound, and every signoff token is content-bound to the complete physical geometry, rules, policy, and report. |
| CS-051 | Accepted | Manufacturing releases use a qualified KiCad CLI as the geometry exporter, require a matching complete signoff token, generate Gerber X2 plus metric drill and IPC-D-356 data, and publish atomically only after independent CAM parsing, manifesting, and checksums succeed. |
| CS-052 | Accepted | Routing stages compose in one direction through immutable typed results: global guides, locked critical copper, general exact copper, physical DRC, then manufacturing; a partial or stale result cannot satisfy a later gate. |
| CS-053 | Accepted | Copper zones and keepouts are typed physical intent. CopperScript owns their deterministic identity, rules, and freshness fingerprint; a pinned KiCad refill is the authoritative manufacturing fill until a native filler passes differential qualification. Stale or missing required fill evidence blocks release. |
| CS-054 | Accepted | Routing and physical DRC share deterministic integer geometry predicates. Interactive shove operations are transactions: recursive movement commits only when the complete result is legal, while locked-object or boundary conflicts return the original immutable board. |
| CS-055 | Accepted | A differential pair is one atomic routed object. Signoff measures coupled and uncoupled portions, skew, and geometrically paired transitions; impedance remains an explicitly external claim. |
| CS-056 | Accepted | Stackups are ordered physical copper/dielectric constructions. Vias select a named fabrication technology whose span, drill, annular-ring, aspect-ratio, and microvia-adjacency limits are validated and exported consistently. |
| CS-057 | Accepted | Fabrication, stencil, assembly, engineering-analysis, and CAM qualification are separate gates. Process limits carry source/revision provenance; engineering results state evidence grade, scope, and validity; unavailable required evidence yields `incomplete` or `blocked`, never an optimistic pass. |
| CS-058 | Accepted | Every normalized hard or external constraint has named downstream consumers and a verifier. Missing ownership or results block release. Soft preferences may affect scoring but cannot weaken a hard requirement. |
| CS-059 | Accepted | Independent CAM tools are identified by exact version and executable digest. Production qualification compares normalized semantics from the required tool tuple over the exact hashed staged bytes; parser disagreement fails and a missing capability is incomplete. |
| CS-060 | Accepted | Coupled route shoves expand collisions by complete net so pair/bus geometry moves atomically. Locked conflicts roll back the immutable transaction; deterministic acute-angle cleanup may replace only a safe degree-two spike and remains subject to physical DRC. |
| CS-061 | Accepted | Differential-pair completion uses bounded local tuning, paired layer transitions, and explicitly bounded return vias. External impedance evidence is content-addressed and suppresses only the corresponding unqualified-model warning; geometric DRC remains mandatory. |
| CS-062 | Accepted | Physical DRC normalizes copper to integer point/segment/polygon spines swept by exact radii, uses a stable spatial broad phase and exact rational narrow-phase decisions, checks normalized zone fills and concave boundaries, and guarantees incremental results equal the authoritative full run. Unsupported copper primitives fail at import rather than being approximated. |
| CS-063 | Accepted | Fabrication, stencil, and assembly DRC derive mask, paste, silk, drill/slot, courtyard, orientation, edge-plating, height, and outer-layer balance evidence from normalized physical assets under a provenance-bearing process profile. Required unsupported artwork yields `incomplete`, and manufacturing profiles may require all three gates to pass. |
| CS-064 | Accepted | Engineering analyses always report status, evidence grade, claim scope, validity limits, and any report digest. Analytical impedance/delay, return-path, DC/via drop and reduced-order thermal results are screening evidence; only explicit content-addressed external reports may claim external-solver evidence. |
| CS-065 | Accepted | CAM qualification adapters parse the exact staged bytes under an exact executable identity. The release profile may require a pinned multi-parser PASS; missing tools/capabilities are incomplete and parser disagreement fails publication. PyGerber is pinned as a semantic parser; subprocess-isolated libgerbv re-exports RS-274X for normalization through the same rendering path. |
| CS-066 | Accepted | `copper.lock` content-addresses local replacements and remote modules alike with a canonical per-file asset inventory. Production uses `--locked --offline`; missing entries, byte drift, symlinks, and remote cache misses fail closed. |
| CS-067 | Accepted | Source `routing` constraints lower directly to `NetRoutingRule`, including critical-net geometry, layers, pair/return-path limits, and content-addressed impedance evidence. Constraint ownership metadata survives semantic and serialized IR. |
| CS-068 | Accepted | Footprint readiness is an explicit whole-board audit that resolves every selected asset, retains its source SHA-256, validates electrical pad mappings, and reports all gaps in one result. Proxy footprints cannot satisfy it. |
| CS-069 | Accepted | KiCad fabrication-only pad apertures are distinct from electrical copper pads. The physical IR retains paste/mask apertures, footprint clearance, heatsink/zone behavior, and unused-layer removal; export, process DRC, geometry fingerprints, and audit preserve those distinctions. |
| CS-070 | Accepted | A CAM-required manufacturing release must freshly qualify its exact tool tuple against a hashed positive/negative corpus before inspecting staged production artwork. The manifest records corpus hashes; a missing tool, missing corpus, unsafe file, parser disagreement, or failed case cannot be published as a qualified release. |
| CS-071 | Accepted | CAM-required release reconciles metric Excellon hits and IPC-D-356 pad/via records against the signed physical IR before publication. KiCad drill and IPC-D-356 coordinates are Cartesian-up relative to PCB/IR Y-down; unknown drill commands and missing, extra, moved, or remapped contacts fail closed. |
| CS-072 | Accepted | Footprint-local copper keepouts are first-class physical IR. They move and mirror with placements and participate in routing, DRC, KiCad export, and geometry-bound signoff; unsupported embedded zones fail closed. |

Changes to an accepted decision require updating this document, its decision-log
entry, relevant tests, and any affected language-reference material in the same
change.

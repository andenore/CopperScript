# CopperScript design specification

**Status:** Draft specification with accepted architectural decisions
**Applies to:** CopperScript language, compiler, IRs, rule engines, and backends
**Last updated:** 2026-10-06

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

An optional, physically numbered package pin may have no electrical profile
while its function is unresolved. It is a structural placeholder only: ERC
must reject any net attached to it. This separates complete land-pattern
coverage from evidence-backed electrical modeling.

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

Reference-layout macros are separate **hard rigid physical clusters**. Their
member poses MUST be bound to resolved footprint geometry and source identity,
with an explicit evidence locator and anchor physical pad/origin. Translation
and permitted rotation MUST preserve all local poses and layer-scoped keepouts.
Fixed members freeze the unit; conflicting fixed poses, unsupported mirroring,
incompatible footprints or overlap between rigid clusters MUST fail closed.
Ordinary legalization, refinement and placement/routing feedback MUST NOT split
a macro. No electrical net/pin mapping changes with a physical transform.

An explicit physical scene MAY bind a content-addressed vendor reference to
resolved footprint hashes and declared physical pad/net roles. This binding MUST
be data-only, reject mismatches and unknown rules, and leave connectivity in the
electrical source. A source locator's local filesystem path is not asset identity.
An explicit internal macro courtyard gap MAY differ from the ordinary additional
gap; non-overlap, external spacing and all copper/fabrication checks still apply.
Provisional footprint adaptation MUST NOT be reported as reference-layout or RF
qualification. See
[rigid-placement-clusters.md](rigid-placement-clusters.md) for the initial API,
bounded-search limitations and separation from RF qualification.

The initial placement engine uses deterministic multi-seed analytical global
placement, hybrid discrete legalization, hard relative-rule repair, and legal
local refinement with coarse per-layer routing feedback. Wirelength and
routability metrics use transformed footprint pads. Multiple legal results are
Pareto-filtered and deterministically ranked; reports MUST retain both the
selected result and candidate metrics. Proxy crossings, vias, pin escape, and
congestion are estimates only and MUST NOT be serialized as routed copper.
High-pin-count packages receive a soft escape-channel spacing objective,
measured between placed courtyards. It may use free board area but MUST NOT
override fixed positions, board/keepout legality, or relative constraints;
close-placement companions move together when a legal cluster translation
improves the score. Spacing remains a placement heuristic, not a routing
guarantee.

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

Board-level `PlacementKeepout` constrains component placement only; it is not a
copper obstacle for global or detailed routing. Only typed copper keepouts may
block tracks and vias. The two keepout classes must not be conflated.

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

Repeated physical lands with one pad number are one logical electrical pin,
but they remain distinct copper objects for layout and independent KiCad DRC.
The PCB flow SHOULD bridge them with exact-clearance copper where legal and
MUST report any unbridgeable lands as pending. Existing exact copper paths on
any common physical layers MUST be reused rather than demanding a redundant
surface bridge. Land closure MUST precede full/subset candidate scoring and
its actual added copper MUST be measured. Native checking MUST include multiple
physical lands even when a net contains just one logical pad reference.
It MUST NOT interpret a shared
pad number alone as proof that all lands are externally connected. A future
explicit, qualified internal-tie declaration may avoid unnecessary copper,
but no such waiver is implied by the current footprint importer.

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
| CS-049 | Accepted | General detailed routing consumes global guides, preserves locked critical copper, owns exact tracks and vias, and reroutes in bounded deterministic passes. Each net's pad-aware grid uses physical-coordinate resource identities. Foreign-net and unconnected pads and locked copper are immutable geometric obstacles. Existing ordinary routes may be tentatively crossed only for a transactional rip-up proposal; the proposal is committed solely if all displaced nets reroute and exact clearances hold. Congestion costs alone cannot legalize overlapping copper. Routing success never substitutes for physical DRC. |
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
| CS-073 | Accepted | Prototype fabrication rules are selected explicitly, never inferred from a footprint or target name. The JLCPCB four- and six-layer profiles currently use their published 0.09 mm minimum copper space and track-width floor while retaining 0.20 mm ordinary tracks. Router acceptance checks exact copper and copper keepouts against those rules; a profile alone cannot certify KiCad project rules, stackup, assembly, or manufacturing readiness. |
| CS-074 | Accepted | Detailed routing builds a deterministic nonuniform search grid that includes connected pad centers as well as regular coarse coordinates. Multiple direction-diverse, geometry-checked pad accesses are search alternatives; only the selected lead-ins become copper. Physical-distance costs and an admissible heuristic prevent irregular grid spacing from distorting route preference. A bounded guide-corridor search precedes bounded unrestricted walk-around; budget exhaustion is explicit and never commits partial copper. |
| CS-075 | Accepted | Source `copper_zone` constraints identify a net and physical layers without adding coordinates to the electrical connectivity IR. Lowering creates deterministic, unfilled physical zone intent with an explicit board-edge inset. The declaration alone cannot close a net or satisfy release: authoritative refill, connected-copper validation, and content-bound signoff remain separate downstream gates. |
| CS-076 | Accepted | KiCad footprint pad positions remain footprint-local, but emitted pad angles include the component placement angle. Export and native DRC must agree on rotated copper shapes; a textually valid board is not proof that rectangular pads remain separated after rotation. |
| CS-077 | Accepted | Every exported KiCad PCB is paired with a same-stem KiCad project carrying the physical IR's minimum clearance, explicit minimum track width, and Default net-class widths/vias. Default widths and via sizes are not mislabeled as global fabrication minima. The project is included in manufacturing staging, checksum inventory, and KiCad DRC; a standalone board checked under machine defaults is not an authoritative signoff result. |
| CS-078 | Accepted | Imported footprint-local keepouts remain nested inside their footprint on KiCad export, with board-space polygon coordinates in PCB artifacts and canonical local coordinates in library artifacts. Board-level keepouts remain board-level. Native DRC exempts the owning footprint's own pads from its local keepout while still checking foreign pads, tracks, and vias; flattening a local keepout into a board rule area changes semantics and is forbidden. |
| CS-079 | Accepted | Until a geometry-aware silkscreen label pass exists, automatically generated reference designators go to the fabrication layer, not silkscreen. This preserves assembly identity without printing overlapping text. Imported footprint silkscreen graphics remain intact; an unimplemented label placement pass cannot be counted as silkscreen signoff. |
| CS-080 | Accepted | Inner-zone pad stitching is a distinct, provisional physical step: exact-clearance surface traces and vias may escape pads into the declared zone outline, but an outline is not a fill. Unstitched pads remain explicit, and even a fully stitched pad set cannot satisfy connectivity or manufacturing release until a pinned fill and connected-copper check prove the resulting plane. |
| CS-081 | Accepted | Physical IR uses board coordinates with Y increasing downward. A positive KiCad footprint or pad angle maps local +X toward board -Y at 90 degrees; native placement, clearance, DRC, and KiCad export must use the same transform. Geometry equivalence is checked against KiCad DRC on routed drafts, not inferred from textual footprint syntax. |
| CS-082 | Accepted | Non-plated footprint drills are mechanical obstacles across every copper layer, including when their pad number is empty. Routing and native DRC enforce the board's explicit minimum copper-to-hole clearance, which is exported to KiCad project rules and bound to the physical signoff digest. The only exception is a `hole_clearance(COMPONENT)` constraint: it applies a smaller, reasoned value between that component's own copper pads and its own non-plated holes, and nothing else. Native DRC records every pair that passes only because of it, the value enters the signoff digest, and the export writes a matching same-stem `.kicad_dru` rule so KiCad's DRC agrees. |
| CS-083 | Accepted | Detailed-route repair distinguishes an exhausted search budget from an exhaustive no-path result. A coarse-grid no-path failure receives one bounded half-pitch retry (not finer than 0.25 mm); budget exhaustion does not inflate the search graph. All retries retain exact copper and hole-clearance checks. |
| CS-084 | Accepted | Multi-pass detailed routing retains two baseline admissible-heuristic passes, then varies weighted-A* guidance deterministically across later passes. Every proposed route still passes exact geometric clearance, and the merge stage accepts only compatible complete alternatives; search-weight diversity never weakens the physical-rule gate. |
| CS-085 | Accepted | Dense SMD fanout is an explicit physical package-access stage before ordinary detailed routing. With fanout enabled, CS-131 moves ordinary local reservations ahead of critical long routes. It uses legal, non-via-in-pad track/via pairs, preserves exact clearance, records unresolved pads, and exposes only verifiable locked-copper via anchors to the downstream router. Fanout is not electrical connectivity intent or a proof of overall route completion. With CS-163 a reserved escape is one terminal of its pad, not its replacement. |
| CS-086 | Accepted | Placement feedback may use exact detailed-routing failures after global routing. It moves only legal, movable components on a bounded trial set, reruns every routing and DRC stage from an unrouted placement, and accepts only a strictly improved completion/DRC score. Existing copper is never dragged implicitly with a component. |
| CS-087 | Accepted | Detailed repair may transactionally rip up a bounded group of ordinary nets, but cannot displace locked critical/fanout copper. It tries deterministic displaced-net orders, rebuilds the clearance state for each trial, and commits only when the candidate and every displaced net reconnect. A failed trial leaves the original route unchanged. Within one routing run, a search repeated with identical inputs (board and checkpoint identity, ordered clearance insertions, net, rule, guide, anchors, congestion, mode and options) may return the earlier attempt from a bounded memo; any changed blocking geometry is a new search, and reuse never changes selected copper. |
| CS-088 | Accepted | Filled-plane connectivity requires version-qualified, same-stem KiCad PCB/project refill and DRC evidence bound to the exact source, exported, filled-board, and report bytes. Any open connection, isolated island, missing report field, tool error, or other DRC violation fails the gate. Provisional zone outlines and pad stitching never count as this evidence. |
| CS-089 | Accepted | Drill-to-drill spacing is checked independently of electrical net, including same-net via pairs and via-to-plated-pad holes. The incremental router and authoritative native DRC share this requirement; copper clearance alone cannot certify hole spacing. An existing same-net via may be reused at its exact position without drilling another hole. |
| CS-090 | Accepted | Pre-escape vias are provisional until a detailed route uses a second copper layer. After routing, an unused fanout via is removed while its connected surface stub may remain; abandoned stubs and vias of failed nets are removed. With CS-163, the land, via and boundary witness copper a successful net does not need is released transactionally and no stub is left open. KiCad dangling-via findings remain a required independent check. |
| CS-091 | Accepted | Global resources estimate usable copper crossings and legal via sites from physical geometry, never from footprint courtyards. A through-via consumes one shared site resource across its full physical span. Pads may offer multiple individually DRC-checked local access candidates; a `region_only` access certifies only its pad exit, not a complete path to the coarse guide center, and is reported separately. Only detailed copper plus DRC can establish connectivity. |
| CS-092 | Accepted | A nearly board-wide inner copper zone reserves that layer for its net. Foreign-net global, critical, fanout, and detailed tracks may not consume it; legal through-vias may cross it. Coarse guides are geometric capacity reservations rather than mandatory detailed-route layers, so bounded detailed search may project a guide across other allowed signal layers while retaining exact via and copper checks. |
| CS-093 | Accepted | Repeated footprint lands with one logical pad number remain separate physical copper objects. A bounded, exact-clearance post-route stitch may join them; separate legal plane contacts may also join them only after authoritative filled-plane verification. Blocked lands are reported, not silently accepted as externally connected. Native logical-pin connectivity and independent KiCad physical connectivity must both pass before release. |
| CS-094 | Accepted | An early plane-pad escape stage can reserve legal surface traces and through-vias before ordinary detailed routing. With fanout enabled, CS-131 places selected plane contacts after ordinary access reservations and critical routing; without fanout, they follow critical routing directly. Signal routing treats these provisional escapes as locked copper. Broad reservation of every GND pad is not the default: it can block a nearby signal or high-fanout power exit. Explicitly selected pads or bounded feedback may reserve only late-failing lands, then rebuild and compare the complete signal route before acceptance. Filled-zone continuity still requires independent evidence. |
| CS-095 | Accepted | A zone net explicitly deferred for filled-copper verification is not a detailed maze-route failure and MUST NOT trigger ordinary detailed-route placement feedback. Actual signal search failures remain trial targets. Pending zone-pad contacts have a separate escape-aware placement objective that can nominate local moves without treating the zone itself as a failed global wire route. |
| CS-096 | Accepted | Provisional surface-pad-to-plane-via escapes first try a short straight or two-segment 45-degree path, then a bounded two-segment Manhattan or three-segment lateral path on the pad side. An arbitrary-angle direct lead-in is a last-resort local fallback, not a long-net routing style. Repeated same-number lands may use the same bounded detour. Every new segment must satisfy exact foreign-copper, keepout, and board-edge clearance; a candidate through-via must satisfy drill and edge rules. Local paths do not assert filled-plane continuity, and unresolved contacts remain pending. |
| CS-097 | Accepted | A logical pad number can identify multiple physically separate SMD lands. Plane stitching must inspect and escape every such land independently, reporting the logical pad as stitched only if all target lands receive legal provisional plane contacts. The electrical IR may retain one pad reference, but that reference alone cannot certify physical continuity of repeated lands; the authoritative filled-zone and KiCad connectivity gates remain mandatory. |
| CS-098 | Accepted | Provisional plane-via candidate grid step and search radius are explicit, reportable routing parameters. Finer search is an experiment, not permission to relax geometry or manufacture rules; only independently refilled KiCad DRC may establish whether it reduces actual opens without adding slivers or other findings. |
| CS-099 | Experimental | A plane-pad escape may use an explicitly selected local trace width only if it is at least the physical IR's minimum track width and clearance floor, never below 0.09 mm, and no narrower than a net-specific width rule. The default remains the ordinary 0.20 mm board track. JLCPCB currently lists 0.09 mm minimum width/space for [six-layer boards](https://jlcpcb.com/resources/6-layer-pcbs); this capability is not a current-carrying, yield, assembly, or CAM qualification. The minimum track width is signed by native DRC and emitted in the same-stem KiCad project; narrow escapes still require independent KiCad DRC and later manufacturing signoff. |
| CS-100 | Experimental | After direct pad-to-via attempts, an opt-in bounded second pass may connect an unescaped zone-net SMD land to a same-side, same-net land already proven to reach a prospective plane via. Only exact-clearance local surface paths within the declared contact radius are accepted; newly rescued lands become anchors for a finite chained retry. This is provisional connectivity only, and filled-zone/KiCad verification remains required. The default radius is zero because the present full-vertical board gained copper without reducing KiCad opens. The contact radius is explicit in route reports. |
| CS-101 | Experimental | A six-layer JLCPCB GND plane escape may optionally place a 0.30 mm diameter / 0.20 mm drill through-via at the center of an SMD GND land when ordinary off-pad access fails. It must satisfy exact all-layer copper, keepout, edge, actual-drill spacing, and drilled-hole-to-foreign-copper checks; any violation leaves the land pending. Such a via is explicitly tagged `filled-capped` in physical IR and counted in the route report. When one exists, the companion KiCad project pins its 0.30/0.20 mm via and 0.05 mm annular minima for independent DRC without relaxing the separate hole-clearance rule. KiCad PCB/Gerber geometry alone does not specify plated-over-filled via processing: the fabrication order must explicitly require it. This is not available for other profiles or non-GND nets, and it does not by itself prove filled-plane continuity or assembly readiness. See [JLCPCB's POFV process dimensions](https://jlcpcb.com/news/free-via-in-pad-6-20-layer-pcbs-pofv). |
| CS-102 | Accepted | General detailed routing uses an eight-heading octilinear search: long straight runs and 45-degree diagonals are preferred through Euclidean step cost and angle-proportional turn cost; a 90-degree turn remains legal when needed. A high-fanout net with at least 16 pads starts in four-heading mode to bound Steiner-tree search growth; a smaller net may also retry once in four-heading mode if octilinear search exhausts its bounded state budget. Each net reports whether this orthogonal mode was used. Only horizontal, vertical, or exact 45-degree shortcuts may replace a detailed path. Exact-clearance and blocked-grid checks remain mandatory, and collinear segments may be merged only without moving copper or erasing a branch junction. Short pin-access lead-ins may have other angles when exact legal access requires them. |
| CS-103 | Accepted | A declared copper-zone net is plane-first by default across global planning, detailed routing, and the `route-board` CLI: it consumes no fictitious global wire corridor and is not maze-routed as a long ordinary net. Global reports explicitly count it as deferred rather than connected or failed. The CLI attempts local pad-to-plane escapes and vias after signal routing; a bounded feedback trial may selectively reserve late-failing pads early. Zone intent and provisional escapes do not establish continuity: authoritative zone refill, connected-copper check, and KiCad DRC are still required. A net merely named `GND` without a declared zone is not silently treated as filled copper. |
| CS-104 | Accepted | Placement geometry and exact legality checks admit explicitly allowed 45-degree orientations; no orthogonal-only normalization may reject such a rule. Unconstrained components retain cardinal default orientations so connectors and assembly-sensitive components are not rotated diagonally without design intent. Candidate generation, transformed pad coordinates, courtyard clearance, region rules, relative constraints, and KiCad export all use the same angle. |
| CS-105 | Accepted | When direct, two-segment, and bounded lateral pad-to-plane-via paths fail, a bounded eight-heading local search may find a short legal off-pad via exit. It checks every edge against exact foreign copper, keepouts, and board edge, checks the via against all spanned layers and drilled-hole rules, and coalesces only collinear segments. Legal coarse via candidates guide A*; the search may inspect finer sites when no coarse site is available. Adjacent same-net lands may share an already committed plane via. The result remains a provisional contact, not proof of filled-plane continuity. |
| CS-106 | Accepted | After the ordinary signal route and late zone-pad escape, a bounded placement/escape feedback pass may target only still-pending zone pads. It tests the nearest local exit, then legal nearby component moves and explicitly allowed rotations from an unrouted placement snapshot, prioritizing components with more pending lands; alternative exit directions follow. Each candidate is preflighted for local contact, fully rerouted with its placement fixed, stitched again, and accepted only if it reduces pending zone pads without worsening the identity of failed signal nets, global/critical status, or hard native DRC. An accepted improvement consumes trial budget, carries its reserved early contacts into subsequent reroutes, and retargets any newly pending pad. A rejected trial never mutates accepted copper. Reports retain each trial outcome and rejection reason. This bounded transactional placement-and-route loop follows the margin/routability principle in [NS-place](https://arxiv.org/abs/2210.14259) and prioritizes package escapes and power access as advised by [TI's AM62x escape-routing guide](https://www.ti.com/lit/an/sprad13a/sprad13a.pdf); it is not an assertion that those methods are implemented verbatim. |
| CS-107 | Accepted | Before moving placement for a late-failing zone pad, a bounded local transaction may remove only detailed-router-created ordinary copper, find an exact-clearance candidate pad escape, query the committed-copper index for its actual blocking nets, and reroute only those nets around the reserved escape. Critical routes, fanout, pads, earlier plane contacts, keepouts, and other ordinary nets stay fixed. Fanout cleanup is scoped to attempted nets; removing input vias requires explicit ownership evidence, and failed subset attempts preserve input escapes. Acceptance compares unaffected-copper identity and fresh complete-candidate ordinary-net connectivity, not cached connected flags. All displaced nets must reconnect, no new ordinary net may open, pending zone pads must decrease, and hard native DRC must not regress; otherwise the immutable original route is retained. Committed metrics are recomputed from actual copper. The final filled-zone and independent KiCad DRC gates remain authoritative. |
| CS-108 | Accepted | Global and detailed routing use soft layer and direction costs in addition to congestion, via, and bend costs. A full-board dedicated plane makes adjacent inner signal copper preferable for long ordinary runs, while surface layers and layers without an adjacent declared reference receive small nonnegative penalties; absent a dedicated plane, no inner-over-outer preference is inferred. With multiple inner signal layers, preferred horizontal and vertical directions alternate. These are optimization hints, not hard restrictions: 45-degree paths, short surface escapes, legal crossings of preferred headings, explicit per-net layer rules, and exact DRC remain valid. If a biased detailed search strands an ordinary signal under its finite budget, it retries with neutral costs and keeps the more complete result. High-speed/RF return-path or impedance signoff is not inferred from a layer preference. |
| CS-109 | Accepted | Planar baseline, layer, and wrong-way search costs use physical length with a shared 1 mm reference, not edge count or configured grid pitch. Integer micro-costs preserve nanometre-scale subdivision invariance; bend and via events remain separately weighted. The baseline planar rate is 10 cost units/mm. Inserting pad-access coordinates must not remove an otherwise valid physical 45-degree successor: octilinear search follows the nearest coordinate-compatible diagonal ray beyond intermediate axis splits, retaining blocked-grid and exact copper clearance checks. Neither a preferred heading nor a grid coordinate is electrical connectivity intent. |

| CS-110 | Accepted | Native explicit-copper connectivity is a layer-aware contact graph of rounded track shapes, placed pad shapes and vias over their actual spans. Positive-area overlap at interior T/cross contacts, pad edges and annuli counts; shared boundaries alone, a nonzero copper gap, a foreign net or an absent common layer do not. Open drills are not solid disks, while explicitly filled/capped vias have surface caps. Repeated pad numbers do not invent a connection between separate lands. Zone outlines and unverified fills never substitute for independent filled-zone connectivity evidence. Optional installed-KiCad differential regression tests exercise these predicates. |
| CS-111 | Accepted | A late zone-escape feedback trial reserves the failing package's entire same-zone-net pin group, including already escaped neighbour pins, before fanout and ordinary routing. Reservations do not extend automatically to unrelated packages or rails. Accepted sequential trials retain those package reservations, compare fresh explicit-copper connectivity, and require independent filled-zone verification. No hard-coded reference or pin number determines this grouping. |
| CS-112 | Accepted | Nonuniform-grid line-of-sight follows physical coordinates. Exact polygon containment detects even narrow concave-outline crossings; physical track-blocking keepouts and on-ray blocked nodes remain obstacles, while off-ray index-space nodes and via-only keepouts cannot remove a track ray. The actual width/clearance query remains mandatory. Cached ray results contain only static outline/keepout/node legality, bound to immutable geometry and axes, not mutable copper clearance. Physical-span via legality is cached only within a single immutable single-net search and never reused after a clearance-index mutation. Neither cache bypasses a check. |

| CS-113 | Accepted | Duplicate-land closure runs before full and subset routing feedback evaluates connectivity. Stitching uses the same exact copper-contact graph as native DRC, reuses existing pad-edge/interior/track/via/multilayer paths, and only joins disconnected physical land groups with bounded, clearance-checked paths on allowed layers. Failed search flags, deferred zone nets and resource overflow are not promoted to success; added ordinary copper and fresh native opens update candidate metrics/status. Native checking includes separated lands on a single-logical-pad net. Final helper reports distinguish surface-pending work from independently verified zone contacts. Only fresh board/export-bound KiCad evidence with zero unconnected items/islands and no non-library violations may resolve pending zone-net references. Library issues remain visible signoff failures; native findings, signoff tokens and fabrication readiness are never waived by this reconciliation. |

| CS-114 | Accepted | Critical copper is committed atomically only after fresh native physical DRC verifies the candidate together with all physical pads, keepouts, board edges, drill envelopes and previously accepted critical geometry. Only unrelated opens and full-route completeness are expected omissions in this early stage; no geometry error is waived. A failed candidate contributes no locked copper. Aligned differential terminals may use a midpoint channel with symmetric 45-degree tapers; non-aligned pair escapes still require joint search, never independent ordinary-net repair. Single-ended critical nets may use bounded exact maze search against immutable earlier reservations, with the original profile rechecked before acceptance. General fanout, subset repair and duplicate-land closure cannot synthesize or prune critical copper. Explicit USB/RF example profiles are geometric intent, not evidence of impedance, antenna performance or manufacturer reference-layout compliance. |

| CS-115 | Accepted | Non-aligned differential terminals use a joint same-layer package escape and heading-aware common-spine search, never two independent maze routes. Legal paired exits preserve member order; coupled lanes use offset-line intersections at straight/45-degree corners. The package taper is actual reserved copper, not an invented straight extension behind its port. Search retries at 1/0.5/0.25 mm with at most eight port combinations and 30,000 expanded states per search at each pitch. Full cross-member geometry, existing critical reservations, original profile budgets and fresh native DRC gate atomic acceptance. Reports retain aggregate searches/states/candidate attempts, including unsuccessful searches. Parallel projected overlap is unioned when measuring coupling so split and unequal mitered edges do not erase or duplicate coupled length. This geometric proxy is not impedance or signal-integrity evidence. New paired layer transitions, non-octilinear pin rows and multi-terminal pairs remain unsupported by this search and fail closed; coarse-guide transitions are proposals, not mandatory copper. Explicitly permitted 45-degree placements are supported without changing electrical pin mapping. |

| CS-116 | Accepted | Reference-layout macros are hard rigid clusters in the physical IR, distinct from soft semantic groups. Local member poses are bound to resolved footprint SHA-256 identities and an explicit source locator, with a unique physical anchor pad/origin. Bounded legalization and refinement transform complete units; fixed members freeze their unit, conflicting fixed poses fail, and ordinary component swaps/repairs cannot split it. Layer-scoped local copper keepouts follow the same transform into routing clearance, native DRC and KiCad export. Rules/templates participate in routing and signoff fingerprints. Front-side translation and explicitly permitted rotation, including 45 degrees, are initially supported; mirroring and overlapping macros fail closed. Reference copper is not transplanted, and no template/source locator is automatic RF, impedance, return-path or fabrication qualification. |

| CS-117 | Accepted | Opt-in physical JSON scenes bind content-addressed reference data to complete resolved footprint digests and declared physical pad/net roles before placement. They execute no template code, fetch no source and change no connectivity. Unknown fields or changed/missing identities fail closed. The machine-local footprint source_path is excluded from asset hashes; source bytes, geometry, library identity and remaining metadata stay bound. Explicit internal macro courtyard gaps never exempt courtyard overlap, external component spacing or copper/fabrication checks. The Nordic three-member example preserves reference midpoint/rotation/physical C3 numbering but is explicitly a provisional footprint adaptation; matching-ground copper, support circuit, antenna and actual stackup remain separate qualification requirements. |

| CS-118 | Accepted | Exact board-edge checks may recognize a four-vertex axis-aligned rectangle and test segment capsules/via disks against its convex erosion using doubled integer distances. Odd-nanometre diameters and boundary tangency retain the original predicate. Only immutable rectangle classification is cached; nonrectangular, concave and invalid vertex orders retain the original exact polygon/edge checks. This changes computation cost, not outline, clearance, search budget or copper acceptance. |

| CS-119 | Accepted | Before accepting a surface-only single-ended critical guide, compare bounded via-free local alternatives on a layer common to every actual physical land and permitted by the routing/plane policy. For two to eight lands, straight/45-degree-first legal paths form a weighted terminal graph; a deterministic minimum spanning tree must include every land, including repeated pad numbers. Clock/RF feed point-to-point limits remain enforced. Original profile budgets and fresh complete-candidate native DRC gate acceptance; a connected incumbent changes only for strictly shorter copper, and failed proposals never displace it or earlier critical reservations. Differential/CAN pairs, global transition ownership and general-net maze routing retain their existing owners. This is not a Steiner solver, source-reference copper transplant or RF qualification. |

| CS-120 | Accepted | Critical routing may emit observational started/finished group notifications. Preflight checkpoints record the running group, completed results and elapsed wall time separately from routing geometry/fingerprints, retaining incomplete status on interruption. Progress/timing telemetry cannot certify connectivity, relax budgets, or promote partial routing to production readiness. |
| CS-121 | Accepted | Opt-in bounded critical placement feedback must use legal whole-unit moves from an unrouted snapshot, rebuild global guides and every critical reservation, and accept only a strict subset of failed critical net identities with no fresh hard native DRC errors. Fixed/rigid/orientation rules, original critical budgets and prior connectivity are preserved. Reports retain rejected trials; this stage cannot certify ordinary routing, impedance or fabrication readiness. |
| CS-122 | Accepted | A joint-search pair may undergo bounded shared-spine compaction/shortcut proposals only with unchanged original package escapes, lane order and endpoint headings. Both lanes are remeasured and checked atomically against original profiles and fresh native DRC with prior reservations. Rejected proposals retain the accepted pair; neither member length may increase. Collinear segment reduction alone cannot be reported as physical wirelength or bend improvement. |
| CS-123 | Accepted | Physical pad coverage and copper connectivity do not validate a component's internal conductive paths or coupled-winding polarity. Source-backed library corrections require topology-specific regressions, updated dependency identities, and fresh routing/electrical review; historical geometry reports must explicitly identify superseded electrical acceptance. Logical signal names must not be reassigned merely to shorten routes. Sources remain optional in the general IR; this decision does not claim a general internal-component circuit solver. |
| CS-124 | Accepted | General detailed routing and its single-ended critical fallback select and emit the same bounded exact octilinear pad-to-grid access path, preserving the actual terminal, net, width and layer. Try both diagonal/straight orders before orthogonal-corner fallbacks; every leg requires board-edge and exact copper/keepout legality. No oblique chord or terminal snapping substitutes for failed access. Tentative rip-up may cross removable tracks but never immutable geometry; its normal transactional acceptance remains mandatory. Other access/zone/fanout owners are not silently rewritten. |
| CS-125 | Accepted | Joint paired offset construction must preserve each spine edge's forward heading in both physical lanes. Adjacent miters consuming a short spine edge may collapse or reverse an inner lane; reject the whole construction and continue bounded search/refinement. Near-goal search may try at most 24 direct/collar tails per state, retaining cheap bridges before one/two-pitch advances and two-leg half/full-pitch S-turn approaches into the fixed end collar. Both original port headings, expanded-state/port-pair limits, full lane/escape clearance and profile/native checks remain mandatory. Never trim one member, move original package escapes or accept stale incumbent provenance to remove a cusp. |

| CS-126 | Accepted | Detailed layer/direction policy telemetry belongs to each net result, retaining requested/effective costs, attempted/selected neutral fallback and the scope of pre-closure failure/overflow comparisons. Accepted subset repairs may have different policies on repaired and untouched nets. Zone deferral has no search policy; missing historical evidence is unknown, not neutral. Telemetry must not alter copper, costs, candidate acceptance, physical metadata/digests or route fingerprints, and is not global-guide policy or connectivity/signoff evidence. Whole-scope neutral fallback remains explicit technical debt until localized transactionally. |

| CS-127 | Accepted | Detailed guide deviation is a physical run cost (50 units per millimetre outside the union of same-layer guide capsules and access squares), including partial boundary crossings, rather than a charge per grid edge. Overlapping regions count once. A via leaving a guided layer for an unguided one incurs one separate 50-unit event; continued movement outside is not another boundary event. Projected-guide search uses projected membership and exposure consistently but never expands the allowed signal layers. Analytic clipping uses floating intersection parameters rounded once per edge to integer nanometres; subdivision differences are bounded nanometre rounding, not additional 50-unit charges. Guide costs remain soft search ordering, not clearance, capacity or connectivity evidence; original hard physical rules and transaction acceptance remain mandatory. |

| CS-128 | Accepted | Ordinary dense-pad fanout enumerates bounded radial track/through-via alternatives against one immutable input, then prioritizes pins with fewer legal alternatives before easy neighbors. Every selection is rechecked against previously selected escapes, including all-layer via, hole-spacing, no-via-in-pad and final native gates; failed proposals cannot mutate input copper. Optional net subsets retain all unselected copper as obstacles. Per-pin initial candidate counts, selected indices and rejection reasons are observational, not onward connectivity, optimal conflict-graph assignment or signoff. Zero-domain pins require a different legal escape representation or explicit placement/owned-copper feedback, not merely a larger maze-search budget. |

| CS-129 | Accepted | Ordinary fanout may expand an empty radial domain into at most 256 off-ray endpoints on a half-step lattice within the existing radius. Both diagonal/straight orders are checked; only the exact legal straight/45-degree lead-in and physical via may be reserved. Detailed routing verifies one/two existing terminal-layer segments meeting exactly from the actual pad center to a same-net spanning via, rechecking geometry; a claimed anchor alone is not evidence. Created track occurrences and vias carry explicit ownership through pipeline, fallback and repair callers. Missing ownership preserves input copper; reused input paths acquire no ownership; failed subset repairs preserve their input escapes. Full cleanup can remove only owned abandoned attempted-net lead-ins and owned vias unused on a second layer. Immutable critical/GND copper, no-via-in-pad and native/fabrication checks remain mandatory. Legal alternatives, initial-domain counts and partial fanout success do not prove compatible assignment, onward routing or signoff. Competing pins with legal radial choices require explicit domain expansion/assignment or placement feedback, not silent clearance relaxation. |

| CS-130 | Accepted | Ordinary fanout retains an immutable-geometry greedy incumbent and may improve only newly proposed selected escapes through exact lazy candidate conflicts and a bounded local CSP. A pending pin and its proven selected blockers may expand off-ray domains with both legal straight/45-degree orders; failed local solutions may grow by one selected outside blocker ranked by consumed-domain pressure. Defaults bound allocation to eight root trials, 12 pins per group, 20,000 attempted assignments per root, 200,000 exact compatibility checks and 2,000,000 total compatibility queries per call. Conservative disjoint-bound tests do not consume the exact-check budget or populate its cache; total queries include cached/broad-phase requests and remain separately bounded. MRV/forward checking, incumbent-first deterministic alternatives and cached conflicts change search ordering, not rules. A complete solution must retain every prior escaped identity and all fixed outside/input copper. Final exact materialization/native acceptance is mandatory; failed improvements revert to greedy, and failed greedy proposals revert to input. Telemetry distinguishes total queries, exact checks, broad-phase acceptance, provisional solutions and final native acceptance. This is not optimal assignment, area connectivity, onward-route cost or authority to displace critical/GND/input copper. Existing multi-leg ownership/anchor contracts must be extended explicitly before adding a different escape representation. |

| CS-131 | Accepted | With fanout enabled, allocate compatible ordinary crowded-pin exits against an unrouted placement before committing critical long routes; specialized critical pair/clock/RF owners must honor those reservations and all original profiles. Explicit early plane contacts follow. Gate ordinary area routing on successful global planning, compatible requested exits, fresh critical connectivity/non-failed status and no hard native finding. Failed preflight preserves diagnostic copper, records zero area-search passes and cannot be bypassed by late plane feedback. Bounded legal placement trials rebuild all stages from an unrouted source and accept only a strict identity-preserving access/critical improvement with no hard native finding. Prior ordinary exits, critical connectivity, fixed/rigid/proximity/orientation rules and ownership remain mandatory; declared 45-degree rotations are allowed. Dogbones are not proof of onward capacity or area connectivity. CS-151 adds bounded owner-order negotiation; CS-152 adds a mandatory provisional ordinary boundary-capacity gate. Joint critical/power/GND allocation, committed layer-aware boundary anchors and directional demand-based margins remain future work in [package-access-first](package-access-first.md). |

| CS-132 | Accepted | USB/differential routing is not inherently restricted to F.Cu. After the coarse-guide route fails, perimeter terminals prefer surface-only joint search, while internal SMD terminal pairs may prefer matched via escapes first (CS-135). Permitted profiles may use matched terminal via pairs and a jointly searched middle spine on another allowed signal layer. Short exact straight/45-degree collars widen to manufacturing-legal via pitch and taper back to trace pitch; explicit transition provenance, equal via counts/dimensions/spans, full physical-span copper/drill checks, required nearby return-net vias, original budgets and fresh native DRC gate atomic acceptance. No independent member repair, imaginary blind via, dedicated-plane signal routing or displacement of input reservations is permitted. The bounded terminal-transition topology supersedes CS-115's statement that new paired transitions are unsupported; general 3-D paired maze routing remains unsupported. Surface-spine refinement cannot remove multilayer transitions. Plane adjacency is a preference, while return-plane continuity, actual layer-specific impedance, via stubs and manufacturing qualification require independent evidence. See [paired-layer-transitions](paired-layer-transitions.md). |

| CS-133 | Accepted | Every KiCad PCB export generates a same-stem project, project-relative fp-lib-table and canonical CopperScript.pretty library from already-resolved physical IR; no export-time downloads or global KiCad configuration mutations occur. Only artifact library identifiers are remapped, with collision-checked geometry/source-identity names; electrical/physical IR and rigid-template identities remain authoritative. Canonical libraries use the same geometry renderer as embedded instances and remain independent of board name/pose. Back-side KiCad geometry explicitly mirrors local Y and adds 180 degrees to the IR's local-X-reflected placement angle, preserving world-space copper; pads, graphics, text and footprint-owned board-space keepout polygons must agree with independent KiCad checks. All CLI, preflight, verification and manufacturing paths write complete projects. Existing unrelated library tables and escaping artifact paths fail before writes; exports never delete old digest-named assets. Manufacturing checksums and verification export digests include every asset. Matching a generated library never certifies source fidelity, antenna behavior or fabrication readiness; unsupported/omitted source features remain explicit warnings. See [generated KiCad projects](kicad-project-export.md). |

| CS-134 | Accepted | Default via generation forbids contact between the complete copper annulus and every pad on its physical span, including same-net and unassigned pads. Soft rip-up treats pads as immutable and cannot bypass this policy. Native physical DRC rejects ordinary same-net via/pad contacts even if KiCad accepts the electrical connection. Qualified filled-and-capped ground via-in-pad remains an explicit opt-in; the complete-board script never enables it implicitly. Ordinary detailed routes and local surface closure may chamfer perpendicular octilinear degree-two corners only after exact copper, keepout and board-edge checks; pad leads, vias, branch contacts and input copper remain fixed. Differential pairs retain joint refinement ownership. A per-layer audit records saved copper shape, overlap locations, sharp bends and coarse density without claiming zone refill, impedance or manufacturing signoff. See [via policy and layer review](routing-layer-review.md). |

| CS-135 | Accepted | After a failed atomic coarse-guide pair route, use physical footprint geometry to order bounded paired searches. If both members and every same-number land at one endpoint are SMD and lie more than one default via diameter plus twice minimum clearance inside the local copper-pad-center envelope on every side, prefer legal matched via escapes before surface maze tiers; perimeter, through-hole and ambiguous terminals retain surface-first search. This is a deterministic ordering heuristic, not proof of a necessary layer change or optimal via count. Pose and component names must not affect classification. Preserve all permitted layers, original search budgets, whole-span pad/copper/drill checks, required return vias, joint refinement and atomic native acceptance. Surface alternatives remain fallback, and the accepted route report records the search order. See [paired layer transitions](paired-layer-transitions.md). |

| CS-136 | Accepted contract; experimental implementation | Reusable partly routed physical hard macros are separate realizations of authoritative electrical modules/component sets, never schematic coordinates or another netlist. Bind pinned assets to exact footprints, pad/net roles and isolated lands; transform poses, immutable tracks/vias, external ports and layer-specific keepouts together, including declared 45-degree rotations. Preserve private same-net returns; routing/rip-up/fill may not shortcut reserved geometry. Verify actual port continuity, layer/technology compatibility, placement/copper legality and ownership transactionally; fingerprints/export/signoff include the macro. Global planning includes immutable occupancy and proved pad/free-track/plated-via boundary terminals while retaining actual netlists; connected internal nets are reused. Package escape skips bound private pads, critical/detailed stages preserve owner copper, and placement feedback rebuilds whole macros from an unrouted source. Source recovery rejects arbitrary non-owner copper/fills. Ground-plane contacts reuse exact private-pad-to-existing-via continuity without claiming filled connectivity. Overlapping private port groups and pre-routed differential macros without paired geometry/return certificates fail closed. Explicit edge margins do not waive copper-to-edge DRC. Mirroring, local pours, automatic multi-port allocation, vendor CAD import and stackup/RF qualification require explicit support/evidence. A connected probe is not an operational or qualified radio. See [physical hard macros](physical-hard-macros.md) and [routing validation](hard-macro-routing.md). |

| CS-137 | Accepted | External library parts, custom footprints and physical assets use the same URL requirement, immutable Git revision and complete content inventory in the managed package cache. Examples and CI do not require a sibling CopperLib checkout or development replacement. Git checkout conversion is disabled for downloaded bytes; canonical local example text is declared by attributes. Physical scenes retain explicit asset/footprint digests and cannot execute package code or rewire electrical IR. Missing/unpublished revisions, offline misses, symlinks and byte drift fail rather than selecting a floating version or silently relocking. |

| CS-138 | Accepted | GitHub Actions attempts the real full-vertical routing flow with pinned tools, bounded budgets and profiling; tags also attempt the nRF52 macro example and publish source/inspection bundles as experimental prereleases. Preserve incomplete boards, reports, per-layer SVGs, logs and available profiles with checksums and explicit failure status. Independent native KiCad refill/zero opens/violations is required for CI completion; artifact publication never claims fabrication readiness or bypasses manufacturing gates. Separate read-only build jobs from the tag-only release writer; do not expose write tokens to untrusted PR execution. |

| CS-139 | Accepted; bounded implementation | Via-in-pad permission may be expressed per semantic component pin and lowered to typed `PadViaInPadRule` physical intent. Never infer permission from land size. Initial support is filled/capped 0.30/0.20 mm centred through-via fallback for explicitly selected SMD GND lands with an inner GND zone and six-layer JLCPCB profile. Reject unsupported targets/processes/profiles and duplicates. Retain full-span copper, other-pad (even same-net), keepout, edge, drill and fabrication gates; qualified finish and manufacturing requirements remain visible in reports/export. A permission is not a required via or proof of filled connectivity. The legacy broad ground opt-in is distinct and is not enabled by the example. |

| CS-140 | Accepted | Ordinary package escape first uses deterministic coarse candidates, then bounded finer radial/two-leg sampling for empty domains and selected-escape conflicts. Expose coarse/refinement steps (defaults 0.5/0.1 mm), cache immutable-domain refinement, retain constrained-pins-first assignment and exact transactional acceptance, and report refined candidates. Refinement never relaxes clearance, pad overlap or macro ownership; finite sampling cannot prove geometric impossibility. Macro router ownership envelopes may be adapted to expose unrelated neighboring terminals without changing source copper or independent fabrication/fill keepouts; pin immutable assets and regression-test real-board access and retained private geometry. |

Changes to an accepted decision require updating this document, its decision-log
entry, relevant tests, and any affected language-reference material in the same
change.

## CS-141 — Permanent component-internal pad connectivity (Accepted)

| Decision | Status | Summary |
| --- | --- | --- |
| CS-141 | Accepted | Explicit permanent package pad groups distinguish assembled connectivity from bare copper; route any usable group land, preserve all solder lands, reject conflicting nets and export KiCad 10 jumper groups. Never infer from duplicate numbers or waive required power/thermal contacts. |

`PartDefinition.internal_pad_groups` contains typed `InternalPadGroup` facts,
referencing package pad numbers without coordinates. A singleton joins duplicate
lands; a multi-number group joins all those numbered terminals. Permanent
installed-component connectivity is explicit, never inferred from repeated
numbers and never applied to `assembled = false` parts. Group membership is
validated against the selected footprint; net conflicts fail ERC/physical IR.
Physical lowering expands otherwise unassigned numbered aliases onto the net.

Assembly-aware connectivity unions these proven groups after actual copper
contacts. Bare copper queries can disable internal edges; neither graph invents
tracks or filled zones. Package escape and detailed access consider alternative
lands and plane stitching needs only one proven prospective contact per group.
Independent refill still proves plane connectivity. Duplicate-pad closure adds
bridges only where neither real copper nor a declared internal path suffices.

KiCad exports preserve explicit jumper groups in project-local footprint assets,
PCB and schematic symbols, conditionally requiring KiCad 10. They are not net
ties or DRC exclusions. Connectivity metadata participates in physical/signoff
identity; empty groups preserve pre-existing hard-macro geometry identity.
Singleton groups are no-ops for single-land proxy geometry, never extra invented
lands. Import errors on invalid fabrication-critical group data are fatal.

Declare a group only if a single external connection is sufficient for its
intended use. It must not waive mandatory thermal, current-sharing or power
contacts. Switch actuation, passive impedance and semiconductor conduction are
not permanent shorts. Via-in-pad remains a separate explicit, process-qualified
constraint. Battery contact via count follows electrical/mechanical requirements,
not an automatic large-pad-area heuristic; the nRF52 prototype retains its
explicit filled/capped BT1.NEG ground-contact exception. Source references remain
optional; generation guidance requires evidence before asserting a connection.

## CS-142 — Real substrate geometry (Accepted; initial implementation)

Board geometry belongs in a separate mechanical/physical description, never
schematic coordinates or a second electrical connectivity definition. One
canonical physical outline, named cutouts and board-owned NPTH holes describe
actual material. Shared exact material predicates gate placement, routing and
DRC; endpoints or bounding boxes alone cannot prove legal copper. Optional
screw-head radius restricts placement on both sides without inventing copper
keepouts. Plated holes remain component pads on ordinary nets.

Validate simple closed topology and reject outside, touching, nested or
intersecting voids. Include geometry in routing/signoff identity. KiCad exports
outer/cutout loops and deterministic BOM-excluded NPTH assets; round Excellon
hits reconcile against the original IR. Export edge clearance explicitly to
match the IR rather than silently inherit a different KiCad default. Board
NPTH process limits are separately provenance-bound, not borrowed from vias.
Initial edge predicates use `DesignRules.minimum_clearance_nm`; independent
profile-specific edge/web/tool limits remain further work.
Manufacturing release rejects the new void features until independent
outline/tooling qualification exists; inspection PCB export remains available.

New-mechanical-geometry filled-zone containment is not yet qualified and fails
closed in physical DRC. Screw-head placement intent is checked in CopperScript
but not yet a native KiCad placement rule. Dedicated mechanical language/datums,
curved boundaries, slots and full manufacturing outline qualification follow
the [mechanical specification and implementation checklist](mechanical-geometry.md).
No schema migration/version bump or fabrication-ready claim is implied.

## CS-143 — Authoritative round outline and bounded query geometry (Accepted)

`BoardOutline.circle()` stores an authoritative `CircularBoardBoundary` and a
deterministic inscribed query ring with explicit maximum radial chord error.
Sampling alone is not proof: all mesh vertices and edges are certified with
integer/Fraction predicates inside the disk and within the requested bound.
Reject mismatched source/ring geometry and unsupported/excessive mesh requests.
Shared material, placement and copper-edge checks use exact disk containment;
older grid/access broad phases may conservatively use the inscribed ring but
must not enlarge real material. Export one native KiCad circle, never facets
that silently replace the intended curved boundary. Geometry and query tolerance
participate in routing/signoff identity. Native filled-zone containment and
independent curved CAM/tooling remain unqualified and fail closed at their
respective gates. This bounded circle support does not imply arbitrary arcs,
rounded paths, slots or curved placement keepouts. See [the round LED example](round-led-ring.md).

On two-layer boards, plane-pad escape may explicitly opt into opposite-side
surface zones. Reserve legal ground escapes before ordinary package escapes
and detailed signals. A same-side pour is not by itself a reason to invent a
via. Existing via/pad exclusions and exact material/clearance predicates still
apply; prospective zone contacts never count as verified filled connectivity.

## CS-144 — Mechanical frontend and generic engine boundary (Accepted)

Board outlines are source intent, not hidden example-builder geometry. A board
may declare one separate `mechanical` block with one circle, rectangle or polygon
outline, polygonal cutouts, named round NPTH holes and physical rule overrides.
Use typed lengths, exact integer-nanometre lowering and source-located topology
diagnostics. Electrical modules cannot contain board geometry. A `Design`
aggregate keeps electrical `Board` and `MechanicalDesign` separate; no outline,
hole or presentation coordinates enter connectivity or schematic IR. Complete
design compilation/JSON and physical CLI commands preserve mechanics; explicit
electrical-projection APIs validate them but return only the electrical board.

Source outlines take precedence over legacy rectangle fallback dimensions.
Generic physicalization applies source fixed poses and side/rotation constraints.
Circle/rectangle zone insets and zero-inset polygons use actual material geometry;
reject unsupported general offsets or intersecting inset/void contours instead
of inventing a bounding-box fill. Native refill remains an independent proof.

`pcbir` contains generic engine code only: no board-specific builder modules,
component fixtures, example imports, example paths or special-case reference
names. Repository-only builders are in `examples/`; concrete bundled fixture
libraries are outside the engine and resolved through a generic library registry
or installed `copperscript.libraries` entry points. Examples are not wheel packages.
The LED-ring source declares its circle, rules, ground zone and all placements.
See [mechanical language specification](mechanical-language.md). Curved slots,
datums and nonrectangular production CAM qualification remain explicitly deferred.

## CS-145 — Exact assembly identities and independent availability (Prototype)

Pin library content and procurement identity independently. An offline assembly
lock binds compiled hierarchical electrical IR to one manufacturer/full orderable
MPN and supplier ordering code per populated component. No implicit substitutions
or inventory-dependent circuit changes. Derive qualified component paths for BOM
consumers without flattening the authoritative IR. Unresolved, unreviewed, stale
or conflicting selections block selection-BOM export; ERC must also pass.

Reusable part identities/offers belong in libraries eventually; the initial
board-side prototype does not alter the electrical grammar. Source references
are optional. Supplier availability is a separately timestamped observation,
not compilation input, reservation or manufacturing signoff. JLCPCB integration
requires approved access and its actual integration documentation; public
catalogue evidence is not an authenticated API call. Keep credentials out of Git.
See [assembly pinning](assembly-pinning.md) for scope, CLI and open release gates.

## CS-146 — Multi-bend package access and native-filled routing closure (Accepted)

Straight/refined elbow escape domains are not complete local-access searches.
An explicit `--fanout-maze` fallback may search a bounded octilinear neighborhood
for a compatible multi-bend launch and legal off-pad through-via. Reuse the exact
surface-path predicates, source track/via sizes, crossed-layer pad exclusions,
joint assignment and whole-board geometric acceptance. Never snap terminals,
relax clearance, or infer via-in-pad permission. Verify an actual endpoint-linked
copper chain and physical via when the detailed router consumes any anchor.

Routing completion and fabrication signoff are different outcomes. An intent-only
zone is not copper in the internal connectivity graph. An exact-board/export-bound
native refill with zero opens and zero violations can complete only genuinely
deferred zone nets after all signal searches, package access and geometric checks
pass. Failed signal searches, stale evidence, waivers and hard findings remain
failures. Do not alter the physical DRC token or mark the board fabrication-ready;
independent CAM, assembly and mechanical production qualification remain separate.

No-path repair may refine only the failed net's mesh in up to four halving
rounds, with a configurable resolution floor (`--minimum-repair-pitch-mm`,
default 0.1 mm). A fixed 0.25 mm floor cannot represent all legal narrow launch
channels. Each trial preserves immutable copper, exact clearance checks and
search-state bounds; exhausted budgets alone do not trigger a larger graph.
Apply that same bounded refinement to soft-conflict proposals and evicted-net
reroutes, not just the final strict search. A narrow launch can be blocked by
ordinary movable copper. Accept a rip-up transaction only after every displaced
net reroutes legally; otherwise roll back all trial copper.
Failed-first passes (third and later) also refine proven no-path searches before
ordinary neighbours are installed. Reserve the difficult legal package channel
early instead of relying exclusively on post-route rip-up to recover it.
Carry the successful candidate's mesh resolution as internal search provenance.
Displaced nets start on that mesh (within the configured resolution floor), not
on a coarser grid that may hide the narrow channel again. This does not change
the physical IR, geometry, clearances, ownership, or transactional acceptance.
After attaching an unanchored, declared internally conductive multi-land terminal, make its
other legal physical accesses available as routing-tree branch origins. Emit
off-pad access stubs only when actually used. Duplicate numbers alone, device
functionality and undeclared internal paths never create such connectivity.
Do not repeat a neutral layer-cost fallback when all layer ranks and heading
preferences are inactive: that is an identical search, not a new strategy.

Expose the physical IR's island-removal policy through `copper_zone` constraints.
Two-sided surface pours may bridge regions separated on one layer; disconnected
fill can be removed with `island_policy = "remove_all"`. This is not a DRC waiver.
Retain useful legal through-via contacts, but when a large land cannot escape,
a declared, non-keepout same-side pour is a prospective contact instead of
forcing redundant via-in-pad. Never use this prospective contact as a surface
chain anchor or filled connectivity proof. Zero opens, islands and other native
violations on the exact completed board remain mandatory for routing closure.

## CS-147 — Native manufacturing files without independent qualification (Accepted)

Support an explicit `--skip-independent-cam` manufacturing-file export path.
Consume the final native PCB and matching project, retain all geometry and
design rules, refill zones, and require zero native DRC violations and opens.
Export Gerbers, plated/nonplated drills, manufacturing netlist and native
positions atomically with checksums and truthful verification status. Circular
outlines and slots are exported by KiCad; independent outline qualification is
not a prerequisite for this explicitly acknowledged path.

Do not weaken or silently bypass the separate qualified-release API. File
generation is not independent CAM qualification, supplier availability or
factory approval. Optional JLC assembly output must use explicit exact BOM
selections and matching placement references, preserving native coordinate
and rotation conventions without guessing supplier corrections. Firmware is
not a manufacturing gate for assembled unprogrammed hardware. All generic
export code belongs in CopperScript; reusable exact parts belong in libraries,
and board-specific choices/build recipes remain in the board project.

## CS-148 — Source-authoritative mechanical and floorplan editor (Accepted)

CopperScript owns a reusable physical-intent editor; board projects own their
`.copper` mechanics and constraints. Derived scenes and generated KiCad files
are not new sources of truth. Rough automatic placement previews must preserve
source/manual locks and rigid macros; applying a result must not freeze all
components. Ratsnest uses explicit physical connectivity islands and remains a
guide, not routing or filled-zone proof. Local UI operations use revision-bound,
validated transactions. Source saves require comment-preserving targeted edits
and conflict detection; until implemented, preview edits are explicitly unsaved.
Electrical connectivity and schematic IR remain unchanged. See the
[editor specification](mechanical-editor.md) and [delivery plan](mechanical-editor-plan.md).

## CS-149 — Importable mechanical board profiles (Accepted)

Reusable physical board standards are declarative `board_profile` package exports,
separate from electrical parts/modules. Boards explicitly apply profiles and bind
existing components to connector roles; importing a profile creates no components,
nets or copper connectivity. Compiled `Design.mechanical` preserves profile
instance paths, namespaced features and source ownership without adding geometry
to electrical `Board`. Exactly one final outline and unambiguous rule/role/pose
ownership are required; conflicts and cycles fail rather than depending on order.
Connector positions anchor to unique physical lands using shared rotation/side
transforms; footprint/anchor/angle mismatches never silently omit a role. Local
source-relative imports remain confined to their source module; external profiles
use the existing versioned, content-authenticated dependency workflow. Imported
features stay read-only in the editor. Reusable production profiles belong in
CopperLib; compiler code contains no board-standard geometry. Mechanical fit
alone is not host electrical compatibility, RF or manufacturing qualification.
See [profile specification](mechanical-profiles.md) and
[implementation plan](mechanical-profiles-plan.md).

### Real host-pattern acceptance example: CM4

The [CM4 carrier](../examples/cm4_baseboard/README.md) exercises two explicit
connector bindings and four mounting holes from CopperLib, composed with a
project-owned outline and antenna keepout. The soldered receptacles are separate
BOM components; the removable module is not an invented additional footprint.
Socket numbering is local (1..100 for each socket), with CM4 pin 101 mapping to
the second socket's pad 1. All fitted grounds and required parallel power
contacts retain individual external connections; no internal-connectivity waiver.

Passive carrier sockets do not stand in for a complete active SoM device model.
Their accepted contact/mechanical facts must not imply validated peripheral mux,
firmware, power sequencing, RF performance or CM5 compatibility. A legal
placement/zero clearance violations is not a routed or production-ready board.
Module body keepouts and antenna copper clearance are separate requirements;
the current socket-corridor representation still needs assembled 3D inspection.

### Generic build workflow

Board projects and repository examples share Make targets and the generic
route/fill/verification runner. Per-example configuration is data (source,
layer/fabrication profile and explicit scenes/options), not Python orchestration
or compiler conditionals. The runner must not assume a particular library URL,
part, board geometry or sibling checkout. CI uses the same configurations as
local builds. Profiling and input/output provenance accompany each routing run;
failure remains nonzero and stale artifacts cannot masquerade as a new run.
Saved filled copper and native DRC on the exact output board are required for
connectivity acceptance, not merely a successful route search or disposable
plane-check copy. This acceptance is not fabrication/assembly qualification.
See [shared Make builds](make-builds.md).

### Advanced mechanical intent

Named datums/edges, component/pad/mating-face attachments, audited body overhang,
height/access envelopes, exact manufacturing curves/slots and locked enclosure
references belong to separate mechanical/physical intent. They never add schematic
coordinates to electrical IR. Conflicting owners/cycles fail, copper-edge rules
are never waived by a body allowance, and unsupported curves cannot invent legal
material. See [advanced mechanical intent](advanced-mechanical-intent.md) and the
editor checklist for the staged implementation/verification contract.

## CS-150 — Footprints as resolvable package dependencies (Accepted)

Part defaults and component overrides may name an exact `.kicad_mod` asset by
GitHub/GitLab module path or `https://` module URL. Versions belong in the consuming
project's `copper.mod`; downloads and complete asset SHA-256 inventories use the
existing managed package cache and `copper.lock`. Layout, routing, audit, export
and editor physicalization must resolve the same inputs without requiring a
download directory supplied on each command.

`footprint-library NAME MODULE/DIRECTORY` in `copper.mod` binds a KiCad namespace
to one directory inside a required module. `NAME:Footprint` selects exactly
`MODULE/DIRECTORY/Footprint.kicad_mod`. Each namespace has one binding; malformed,
duplicate or unprovided bindings fail. The project declares the provider's
version explicitly. Missing assets or lock drift never select a different source.

Relative `.kicad_mod` references authored inside imported parts or modules are
relative to that declaring package and confined to its module. Lower them to
portable module identities (or entry-source-relative paths for workspace imports),
preserving package ownership across aliases, nested modules and serialization.
Package assets cannot accidentally bind to an identically named board-local file.

Exact URLs always use managed resolution. Explicit local roots may override a
namespace only in unlocked development, with local provenance; a locked managed
namespace verifies its pinned source. Existing unbound local references and legacy
managed `footprints/` lookup remain supported. Ambiguity, declared-name mismatch,
pad mismatch, symlinks and module path escapes fail; imported geometry remains
subject to the existing physical validation rules.

`copper lock BOARD` prepares selected managed footprint dependencies as well as
source imports. `--locked` rejects missing/changed dependency inventory and never
updates it; downloads remain allowed when matching content is missing from cache.
`--offline` prevents remote fetching and requires local/cached sources. Offline
alone may create/update a lock. Explicit local files outside required modules
are not covered by the package inventory. Provenance identifies which route was
used, the selected file digest and managed module/revision/inventory when present.
See [footprint dependency syntax](footprint-dependencies.md) and
[implementation plan](footprint-dependencies-plan.md).

## CS-151 — Transactional cross-owner escape-pattern negotiation (Accepted)

Before placement repair, failed ordinary, critical or selected early plane access may
propose alternate ordinary escape patterns against specialized critical and
provisional plane owners. Discard probe copper, rebase only fanout-owned stubs
onto the original macro-materialized source, then rebuild critical routes and
selected plane contacts under their unchanged profiles. Accept only strict
identity-preserving failure reduction, every previous ordinary exit/required
pin retained, no new pending contact or failed critical net, and fresh native
geometry acceptance. Otherwise preserve the entire incumbent. Never reroute
differential members independently, displace hard-macro copper, relax geometry
or SI profiles, or treat an early plane contact as filled connectivity.

Bound the implementation to at most two deterministic owner-order proposals
per search tier (CS-154); ready preflights do no negotiation. Keep proposal/revalidation
outcomes and timing events explicit. This is a repair heuristic, not an optimal
simultaneous-escape solver, proof of onward capacity, or manufacturing signoff.
The unchanged fail-closed CS-131 gate remains mandatory before area routing.
See [package-access-first](package-access-first.md#cross-owner-pattern-negotiation).

## CS-152 — Provisional ordinary boundary-capacity gate (Accepted)

With fanout enabled, ordinary area routing MUST also require a jointly compatible
local path beyond a package collar for every allocated ordinary launch. Recheck
actual pad-to-via connectivity and source/copper ownership, use transformed
courtyard/body and land geometry, and restrict witnesses to allowed signal
layers reached by the actual via. Through-vias occupy their full physical span;
dedicated planes, hard macros, holes, keepouts and original manufacturing rules
remain mandatory obstacles/constraints.

Generate bounded exact straight/45-degree paths with anchor-local adaptive
sampling and a bounded heading-aware multi-bend fallback. Select compatible
domains with explicit CSP budgets, then temporarily materialize and native-check
the entire witness set. Independent pin success or a width-only capacity count
MUST NOT certify a shared channel. Unproven access joins pending identities and
blocks area routing with zero passes; finite-domain failure is not impossibility.
Pattern/placement acceptance MUST preserve previous boundary proofs by identity.
Incremental placement MUST rebuild evidence or request a full preflight fallback.

Witnesses are diagnostic physical objects, not electrical/schematic IR data and
not silently committed copper. They MUST be reported as provisional and MUST NOT
claim end-to-end connectivity or fabrication readiness. Layer-aware boundary
anchor ownership (implemented by CS-153), joint critical/power/GND domains, directional placement margins
and independent filled-board signoff remain follow-ups.
See [ordinary package boundary access](package-boundary-access.md).

## CS-153 — Owned layer-aware ordinary boundary hand-off (Accepted)

After the entire CS-131/CS-152 preflight passes, atomically materialize the
complete ordinary boundary witness set as fanout-owned copper. Bind the proof
to its physical source digest and unique launch identities; recheck transformed
collars and port clearance beyond their edges, and verify existing
owned launch occurrences, exact terminal/path continuity, actual via span,
per-net allowed layers/widths and fresh native geometry. Invalid arguments MUST
fail without changing input geometry. Blocked preflights MUST NOT commit witnesses.

Detailed routing and subset repairs consume explicit selected-layer ports,
not unrestricted via-center aliases (with CS-163, as one of the pad's verified
terminals). Every claimed path MUST exist and validate;
missing or stale access MUST NOT silently revert to pad-center routing.
Preserve dogbone identities separately for upstream pattern negotiation.
Only newly appended ordinary track occurrences acquire cleanup ownership;
critical, macro, plane and existing input copper remain protected. Abandoned-net
cleanup is occurrence-based. A surface path may retain connectivity without an
unused owned via; an off-surface port requires its real transition.

Placement transactions MUST rebuild access evidence and ownership. Until exact
incremental rebasing is implemented, nonempty committed boundary reservations
request full preflight. Reports distinguish provisional proof from materialized
reservations and record commit-time cost. Local paths remain physical objects:
they neither add schematic coordinates nor certify end-to-end routing, filled
plane continuity or fabrication readiness. Specialized critical/GND domains and
joint dogbone/onward-path optimization remain separate follow-ups.

## CS-154 — Early negotiation with conservative full-search fallback (Accepted)

When paired critical nets and pattern negotiation are enabled, package preflight
first explores a bounded search tier. The default aggregate expansion cap is
6,000 states per pair per critical pass, shared across surface/via families,
existing pitches, layers and candidate ports. Bounded slices sample both
families/pitches; clean-owner probes and final pattern revalidation use the same
cap. Standalone critical routing keeps its historical bounds by default.

The CS-131/CS-151/CS-152 acceptance gates remain unchanged. Limited failure is
not proof of impossibility. If the initial tier is not ready, retry the ORIGINAL
ordinary pattern with historical full budgets and bounded pattern negotiation.
Select full fallback only if it preserves all incumbent access identities and
does not introduce new pending contacts, failed critical nets or hard findings.
Otherwise keep the incumbent and report the unsuccessful fallback. Each tier
may propose up to two patterns; no unbounded same-budget retry loop is allowed.

`--package-initial-pair-states 0` disables staging. Disabling pattern negotiation
also retains the historical full-budget path. Reports/progress expose tier,
aggregate cap, exact expanded-state exhaustion, fallback and selection. Work
counters include discarded clean probes and rejected proposals, not only the
selected critical result. Counters are telemetry, never connectivity evidence.

## CS-155 — Immutable bounds and static clearance broad phase (Accepted)

Rounded shapes compute their bounds once per immutable instance. The cache must
not affect equality, hashing or representation; copies, serialization by pickle
and replacements must preserve/rebuild valid bounds. Exact integer/Fraction
distance and clearance predicates remain authoritative.

Routing clearance indexes keepouts and protected macro regions by layer and
coarse spatial bins. Deduplicate candidate identities in deterministic original
order. Bound memory/work for huge envelopes with conservative per-layer lists.
Broad-phase queries must include the macro one-nanometre contact exclusion.
Preserve full physical via-span checks, object flags, transformed footprint
geometry, mechanical clearance and globally fail-closed unsupported keepout
holes. Compare answers against the prior linear predicate implementation.

These caches belong to one shape/board snapshot, not a global XY-only legality
cache. New placement/rule/mechanical snapshots create fresh indexes. Newly added
copper continues to update a separate mutable spatial index. No stale legality
answer may be reused after copper insertion or placement changes.

See the [implementation and validation plan](routing-search-speed-plan.md).

## CS-156 — Directional escape-aware placement (Accepted)

Physical placement SHOULD reserve facing package-access corridors from actual
connected-pad demand, copper/drill pitches and net/layer eligibility rather than
requiring both neighbours to exceed a pin-count threshold. Power and ground
access MUST participate. Signal-layer count MUST NOT multiply surface via-bank
capacity. This estimate belongs to physical planning, not electrical IR.

Channel deficits SHOULD rank ahead of wire-length improvement, while hard
mechanical, fixed-pose, rigid-macro and proximity constraints remain unchanged.
Move constrained units together where possible. Unresolved corridors MUST be
reported as provisional warnings, not silently override locks or claim a
fabrication violation. A heuristic-clear placement MUST still pass the existing
joint package-access reservation before ordinary area routing.

No specific example component or board coordinate may be encoded in the generic
placer. Dedicated power-plane selection is a separate board policy, not a
consequence of this spacing heuristic. See the
[implementation plan](escape-aware-placement-plan.md).

## CS-156 — Derived physical power domains (Accepted)

Physical planning MAY derive `PhysicalPowerDomain` membership from explicit
nonzero supply nets and active power-pin profiles. Net identity, not nominal
voltage or device-local domain name, determines membership. Same-voltage
switched/filtered nets remain separate; aliases on a net merge. Ground is not
a placement domain. Supply output pads and explicit physical sources anchor
distribution; absent sources use a consumer centroid. Passive rail terminals
participate and a multi-supply component MAY belong to several domains.
The view MUST remain a subset of actual physical net terminals and MUST NOT
create connectivity or geometry in the electrical IR.

Domain distribution is a configurable, component-normalized soft objective.
Actual supply-pad position/orientation matters; source locks, mechanical and
relative legality, macro ownership and escape-channel deficits remain stronger.
No mandatory geometric midpoint or analog/digital ground split is inferred.
Plane-backed nets use reduced generic wirelength attraction. Report raw HPWL,
weighted wirelength and the domain penalty separately. This is not power
integrity, current capacity, sequencing or voltage-drop certification.

## CS-157 — Complete small-package and plane access (Accepted)

Crowded ordinary SMD terminals on multi-terminal packages MUST NOT be excluded
solely because a package has fewer than twelve pins. Default fanout includes
small packages with at least three distinct electrical terminal numbers when
local spacing is within its configured threshold. Two-terminal passives remain
excluded by default; explicit thresholds retain bounded custom opt-in. Keep
critical/macro ownership, exact candidate and final native gates unchanged.

This decision amends the opt-in/late-plane defaults in CS-094, CS-103 and CS-131:
with fanout enabled, reserve requested declared-plane contacts during package
preflight by default, before ordinary area routing. Critical routes retain
their specialized profiles; cross-owner pattern proposals are revalidated from
the clean source. Explicit late-contact opt-out and selected-pad scope remain
available. Late feedback MUST preserve the identities of already accepted
early contacts when rebuilding placement. A failed gate cannot be bypassed by
area routing. Prospective contacts are not proof of continuous filled copper.

## CS-158 — Private branch checkpoints for repair (Accepted)

A failed multi-terminal detailed attempt MAY retain its successful tree as a
private checkpoint for bounded repair. Checkpoints MUST bind source identity,
grid coordinates/layers, terminal identities, access anchors, width and allowed
layers. Revalidate old geometry against the current clearance index before
reuse; soft proposals may cross only movable owners. Resume only remaining
terminal branches and expose reused-branch telemetry. A changed source or mesh
invalidates reuse. Checkpoint copper and congestion MUST NOT appear in output
on failure. Complete candidates still require exact clearance and restoration
of every displaced net; immutable fanout/critical/source copper is protected.
This is not permission to publish a partial power tree or to rip arbitrary
branches from previously completed blocker nets.

## CS-159 — Explicit regional power pours (Accepted)

Source `copper_zone` MAY use an explicit rectangle, a millimeter polygon or a
named placement-region boundary, with a nonnegative integer priority. Boundary
modes are exclusive and independent of electrical net definitions. Default
whole-outline/inset behavior remains available. Validate simple polygons,
board containment and cutout relationships; reject unsupported offsets and
partial cutout clipping rather than approximate them. Native authoritative
fill/connected-copper signoff is unchanged. Do not automatically assign a power
layer, construct regions from consumer bounding boxes or split GND. Distant
loads still need legal access; regional intent alone cannot defer away opens.

An explicit `reserve_routing = true` MAY reserve the polygon for its net's
tracks on the named layers. It defaults to false. Foreign track envelopes
respect board, zone and both net clearances; owner tracks remain subject to
normal DRC and hard-macro ownership. Through-vias are not blocked by this
intent and require normal native antipads. Distinct-net reservations on a
common layer MUST NOT overlap or touch. Shared access/routing/repair/cleanup
checks and final physical DRC enforce reservations; global guides exclude
unavailable planar resources per net. No blanket native copper keepout is
exported, and neither reservation nor contact intent proves filled continuity.
Reserved general-net regions retain crowded package exits through joint
fanout and ordinary-routing cleanup. Plane stitching tries an existing
nearby via with a width/clearance-checked tail before adding a new drill, within
its existing search radius. Unreserved zone nets remain excluded from fanout.

See the [implementation and validation plan](power-domain-routing-plan.md).

## CS-160 — Explicit shared-reference return topology (Accepted)

Required paired return vias remain mandatory by default (`always`). An explicit
`reference_change` policy with a typed `shared_reference_layer` MAY omit a via
only when both actual signal-contact layers are adjacent to the identical
declared reference plane. Both member policies and return nets must agree;
ambiguous, absent, different and nonadjacent references retain the required-via
behavior. Source annotations, not component names or signal names, choose this
policy. Keep the whole emitted pair inside a single unperforated declared
reference-zone outline and reject projected zone keepouts.

The critical owner MUST recompute signal contact layers from emitted copper,
not a through-via's barrel endpoints, and validate the complete pair atomically.
Bind the policy and reference layer into physical/ownership digests and report
shared-reference transition counts. A declared shared reference is not evidence
of refilled copper continuity, controlled impedance or USB/RF performance.
Native refill/all-severity DRC and manufacturing gates are unchanged. Never
remove mandatory vias under the default policy, add dummy copper merely to
suppress a warning, or suppress `via_dangling`. See the
[implementation plan](shared-reference-return-plan.md).

## CS-161 — Crossing- and demand-aware ordinary layer assignment (Accepted)

After negotiated global routing, an ordinary (`GENERAL`) net's via-bounded
planar guide run MAY be relabelled to another permitted signal layer when that
lowers its soft cost: the unchanged rank/heading preference, one via pair per
forced same-layer crossing (two guides passing straight through one tile in
perpendicular directions) and an optional local-demand cost against detailed
lanes. The pass MUST keep every tile path, via position and count, access pad
and the global quality vector; it MUST NOT move dedicated planes, explicit
`allowed_layers`, critical/paired/power guides, non-via pin accesses or
inner-to-inner transitions, merge same-net runs, or exceed edge capacity.
Detailed corridor search MAY admit the projected corridor on any permitted
layer within a configured radius of a fixed-layer terminal, so a reserved port
can change to its guide layer there. Both mechanisms are opt-in: while every
ordinary package terminal is a rank-0 boundary port, forcing other layers adds
transitions beside package collars and destabilized full-vertical closure.
Guides remain intent, never clearance evidence; exact checks and acceptance
gates are unchanged. Spare-looking layers are not qualified reference planes.
See the [layer-balance review](routing-layer-balance-review.md).

## CS-162 — Ownership-safe smoothing of accepted ordinary copper (Accepted)

After detailed routing and owned-stub pruning, owned ordinary track chains MAY
be straightened: a sub-chain between anchors (vias, branches, width changes,
immutable copper and interior contacts) is replaced by one straight or one
45-degree-plus-straight connection only if it is never longer, never sharper,
and shorter or less sharp overall. Every new segment MUST pass exact clearance,
keepout and outline checks; every pad and via-layer contact of the retired
copper MUST remain; a net whose explicit-copper islands or via contacts would
change keeps its original copper. Vias, immutable, critical, reserved boundary
and zone-net copper are never inputs. The library default is off; the board
router enables it unless `--no-route-smoothing` is given.

## CS-163 — Escape terminals and release of unused escape copper (Accepted)

A materialized ordinary boundary access (CS-153) MAY be one terminal of its pad
rather than its replacement. With `escape_terminals` (library default off; the
board router enables it unless `--no-escape-terminals` is given) detailed
routing also offers grid nodes on the verified land and witness centre lines and
the launch via on each permitted layer it spans, which need no lead-in, and the
pad's own exact surface access candidates. The escape stays reserved for the
whole routing run and the pin keeps its constrained-first priority, so a
genuinely constrained pin keeps a legal exit. Once one terminal of the pad
connects, the others are virtual tree roots, materialized only if a later
branch starts there; a branch walk that passes a tree node starts from the last
such node instead of closing a loop. Unverified access still fails closed.

After routing, owned escape copper a successful net does not need is released
transactionally, per access: land, via and witness; else via and witness; else
the witness. A bounded pass then removes disposable branches that close
same-net loops. Each trial simulates cleanup (pruning owned dead ends, including
those beyond a land or via centre, and deleting new or owned vias left on one
layer) and is kept only if, judged on copper shrunk by 1 µm, the net stays
connected and gains no island, open track end or single-layer via. Only owned
occurrences of that net change; critical, macro, plane, input and other-net
copper never do. Retained escape copper may lose dead-end tails but is never
smoothed (CS-162). The CS-131/CS-152/CS-153 readiness gates, exact clearance,
ownership and rollback are unchanged. A released escape is not a stale
reservation: later subset reroutes treat that pin as an ordinary surface
terminal. Only nets with a reserved escape are affected.

Escape selection is unchanged: every escape is now a fallback dropped if unused,
not proof that an unreserved pin would have routed. Destination-facing boundary
ports (`--package-destination-ports`) remain opt-in because they lengthened
routes on the iteration board. See R18 in the
[routing review checklist](routing-review-todo.md).

## CS-164 — Neck-down of ordinary nets near terminal lands (Accepted)

An ordinary (`GENERAL`) net whose rule declares breakout properties (D-PHY plan
R1) MUST use `breakout_width` and `breakout_clearance` only inside a terminal
land's breakout region, and `width` and `clearance` elsewhere, as the critical
router does. Detailed search edges and line-of-sight shortcuts, pad access
paths (shared with fan-out and boundary escapes), escape verification and
global pin access check every centreline as its `BreakoutRegions.split_tracks`
pieces and emit exactly those pieces, so an edge crossing the boundary is
narrow up to the region edge and wide beyond. Copper is never emitted narrower
than it was checked: collinear runs merge only when the result stays outside
every region or inside one, a necked corner is chamfered only inside its
region, smoothing checks and emits new segments as pieces with width changes
as anchors, and stub pruning joins pieces of one centre line that touch only by
width across a neck-down cut. After pruning and escape release, changed owned
copper is re-cut so a remnant inside a region carries the breakout width; a net
whose explicit-copper islands or via contacts would change keeps its copper.
Global-routing demand, placement escape estimates and the package-escape maze
keep the full width (conservative). Nets without breakout properties MUST route
byte-identically. See R19 in the
[routing review checklist](routing-review-todo.md).

## CS-165 — Planned paired layer swaps for crossing bundle pairs (Accepted)

When the pair order at the two ends of a critical bundle is inverted, the
critical router MUST plan the crossings before routing the bundle, not leave
them to coarse-guide via proposals. Pair ranks are read by angle around each
component's courtyard centre in one rotational sense, starting away from the
other component; two pairs cross when their ranks disagree. The surface keeps
a non-crossing set with as many pairs that cannot swap as possible, then as
many pairs as possible; every other pair gets one paired layer swap on an
allowed layer (of its `layer_group` where declared, with an adjacent declared
zone where any qualifies), never the layer of another moving pair it crosses.
Moving pairs route first, using only the existing paired-via proposer on that
layer: matched vias at `transition_spacing`, return vias or a declared shared
reference (CS-160), no transition via in a breakout region, the reserved
corridor first. Every candidate passes the unchanged profile, plane and atomic
native-DRC gates. A member whose `max_vias` is below 2, or a pair with no
layer left, is reported impossible and fails without a surface or coarse
fallback. The bundle record lists each crossing, its layer, transitions and
return-via or shared-reference decision. Bundles without crossings MUST route
byte-identically. A planned layer is not impedance or via-stub qualification.
See R5 in the [D-PHY routing plan](dphy-routing-plan.md).

## CS-167 — 45-degree corners on tuning bumps (Accepted)

Length-match bumps (R3) and intra-pair skew bumps (D5) MUST NOT have 90°
corners. A bump keeps its perpendicular legs and chamfers every corner at 45°:
for height h, track width w and d = ⌊(2 − √2) × lane spacing⌋ for a pair (0
for a single net), the member inside a turn takes chamfer leg
c = min(w, ⌊h/4⌋, ⌊(h − d)/2⌋) and the other member c + d, the offset chamfers
of a coupled 45° bend. Every 45° piece has equal integer dx and dy, and
parallel pieces of the two lanes are never closer than the lane spacing (at
most 2 nm farther). A bump too low for c ≥ 1 has one 45° ramp per side. Each
pair member is inside the turn at two corners of a bump and outside at the
other two, so both gain exactly the same length and the pair's skew is
unchanged. Tuners MUST count the chamfered gain as critical lengths measure it
(each piece rounded on its own) when choosing bump counts and levelled
heights, within the amplitude and bump limits; on boards with breakout regions
their targets stay 32 nm inside `max_skew`, because re-cut 45° pieces may
round differently. The R3 room check uses the convex outline of the chamfered
bump; native DRC still validates every tuned candidate atomically. Boards
without tuning MUST route byte-identically. A chamfer is not a qualification of
the bend's impedance. See R9 in the [D-PHY routing plan](dphy-routing-plan.md).

## CS-168 — Routed critical-lane review (Accepted)

The route report MUST review the exported copper of every net with a critical
(non-general) routing rule, and the critical preflight report the critical
stage's copper, so lanes can be checked against layout guidance from the
report alone. Per net: the lane-table length (tracks only, via barrels
excluded, as length matching), layers and own vias; per pair, skew against the
smaller member `max_skew`; per `length_match` group, the verified skew against
`max_skew` with its tuning outcome. Spacing is the least edge-to-edge distance
from the net's tracks and vias to another net's tracks, vias and pads on a
shared layer, split by the net's own copper inside or outside its breakout
regions (`BreakoutRegions`) and by critical or other signal neighbours, with
neighbour, object, layer and location. The pair partner, pads without a net
and nets that own a copper zone are ignored rather than reported as a third
class: planes and pours are reference copper, not aggressors. Coupled length
is a pair member's track length outside its breakout regions that lies closer
than 2 × `pair_gap` to copper of another critical pair. Bends are measured
where exactly two of the net's tracks meet on one layer; those over 45° are
listed. Neighbours come from the `RoutingClearanceIndex` bins within 1 mm,
never all pairs of objects; farther copper is not reported. The review is
report-only: routing, boards and every other report section MUST be
byte-identical. It is geometric screening, not crosstalk or impedance
qualification. See D6 in the [D-PHY routing plan](dphy-routing-plan.md).

## CS-171 — Nested exits at package corners (Accepted)

When a critical bundle leaves a package across a corner, its side-edge pairs
MUST NOT run past their far end's column and back. A pair's edge at a
component is the outward side of its two lands (one row along x or y, outward
away from the component origin). A side pair exits perpendicular to the
direction in which it enters the far component, with the far lands ahead in
both; the bundle wraps a corner when another of its surface pairs leaves along
that direction. Side pairs with one exit and travel direction form a nest,
innermost (nearest the corner) first; a nest whose far columns are out of
order (its pairs cross, CS-165) is not planned. Each pair plans its run on its
far column, or, when that is nearer, just outside the pair inside it (both
half-bands plus the larger clearance), and never nearer than the shortest
port, a one-pitch diagonal and a lane offset. The nest takes its slots in the
bundle order innermost first, so each outer pair routes around the inner
pair's actual copper. The router tries the nested exit before any other
candidate: the shortest legal port, one 45° diagonal onto the run (the longest
that clears committed copper, down to one pair pitch), the run, and a 45° jog
back just before the far port when the run lies beyond the column. A run that
does not clear, or whose skew-tuning bumps do not, steps outward by 50 µm, at
most 2 mm, within 256 tuned candidates; at most three take the unchanged
profile, plane and atomic native-DRC gates. Otherwise the pair routes exactly
as before. The bundle record lists each nested exit with its column, planned
and routed run and diagonal, and whether it routed or fell back. Bundles
without a corner wrap MUST route byte-identically. See R7 in the [D-PHY
routing plan](dphy-routing-plan.md).

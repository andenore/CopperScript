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
- package-independent device capabilities, peripherals, and pin-mux options;
- component instances and selected footprints;
- explicit peripheral and pin-mux selections;
- nets and their endpoints;
- supplies, nominal voltages, and power sources;
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
types, limits, manufacturer data, and compatible physical packages.

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

### 3.9 Complex devices separate capabilities from packages

Complex programmable parts such as MCUs MUST distinguish three concepts:

- a package-independent `DeviceDefinition` describing silicon peripherals,
  peripheral signals, mux options, and shared configuration resources;
- a `PartDefinition` describing one orderable/package variant and the physical
  pins actually bonded out by that package; and
- a `PeripheralSelection` describing the explicit mux choices made for one
  component instance.

A mux option maps one physical pin identity to one peripheral signal and
retains the target-specific selector needed by a backend, such as `AF6`.
Optional resource and setting fields express device-wide configuration state.
Selections requiring different settings for the same resource conflict;
multiple signals requiring the same setting are compatible.

Peripheral configuration annotates component behavior and MUST NOT create or
modify net connectivity. A configured pin is connected only by appearing as a
net endpoint. This preserves nets as the single source of electrical
connectivity.

v0.1 requires explicit pin assignments. Automatic pin assignment MAY later be
implemented as constraint solving, but its result MUST lower to the same
concrete `PeripheralSelection` IR and retain every selected pin and mux
selector. Device packages are declarative data and SHOULD be generated from
traceable manufacturer data where practical; provenance such as source and
revision SHOULD be retained as device metadata.

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

The first external backend will generate a KiCad schematic as an
interoperability artifact. Its goals are to:

- validate that CopperScript electrical semantics map to a real EDA tool;
- provide a visual, inspectable schematic;
- carry references, values, footprints, nets, and labels into KiCad; and
- support KiCad's normal schematic-to-PCB workflow.

It is not a layout engine and MUST NOT introduce schematic coordinates into the
electrical IR.

The initial implementation SHOULD:

- target one explicitly named KiCad file-format version;
- use a separate mapping from CopperScript parts to KiCad symbol and footprint
  library identifiers;
- create deterministic UUIDs from stable semantic identity;
- use a simple backend-local grid arrangement;
- prefer named net labels over complex aesthetic wire routing; and
- verify generated artifacts with fixtures and, where available, KiCad tooling.

The generated schematic is not the source of truth. Round-trip import and
back-annotation are outside the first implementation and require a separate
design decision.

### 6.2 Future KiCad PCB backend

A KiCad PCB backend should consume the physical IR, not schematic drawing data.
It will serialize placement and routing decisions while preserving references
to electrical nets and components.

Generating an unrouted board skeleton directly from the electrical IR MAY be a
useful intermediate feature, but any automatically chosen placement belongs to
backend output or physical IR, never to the electrical IR.

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

- automatic PCB placement and routing;
- copper pours, vias, stackups, and fabrication output;
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

Current footprint strings are demonstrations rather than normalized library
identifiers. The KiCad backend should introduce an explicit mapping layer
instead of redefining those strings as implicitly KiCad-specific.

## 10. Open design questions

These questions are intentionally unresolved:

- Should stable identities be explicit in source or derived from qualified
  semantic paths?
- Should stable hierarchical identities remain path-derived if instances are
  renamed, or should source-level persistent IDs be introduced?
- Can separate constraint-profile files override or extend inline constraints?
- What is the serialized schema for the future physical IR?
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

Changes to an accepted decision require updating this document, its decision-log
entry, relevant tests, and any affected language-reference material in the same
change.

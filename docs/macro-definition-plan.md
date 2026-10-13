# Copper hard-macro definitions

Requirement: author reusable physical hard macros in `.copper`, export them as
named package symbols, and select them with the existing `physical_macro`
binding. Dependency versions belong in `copper.mod`; footprint and asset hashes
are generated from verified inputs. Electrical modules remain authoritative.

## Contract and implementation plan

- Add a standalone `macro NAME { ... }` document, one definition per file, with
  source-located declarations and exact engineering quantities.
- Declare net roles, footprint members, explicit pad assignments and isolated
  pads. Footprint imports name exact `.kicad_mod` assets; literal footprint
  references also use the existing footprint resolver.
- Represent every current v0.5 asset feature: member poses, anchor, allowed
  rotations, layer requirements, tracks (including pad endpoints), vias and
  finishes, ports, protected regions, keepouts, zones, fixed polygons, plane
  returns with dedicated contacts, named width contracts and edge openings.
- Reject unknown properties, duplicate identities, unresolved references,
  dimension/type errors, lossy nanometre conversion and unsupported features.
  Definitions cannot create electrical connections or assert qualification.
- Export `alias.Definition` through ordinary package imports, including local
  packages, managed dependencies, replacements and locked/offline resolution.
  Retain legacy JSON imports and scenes.
- Lower deterministically to the existing JSON schema and immutable binder.
  Generate exact footprint geometry digests; retain source/footprint input
  fingerprints and recheck them before binding and evidence replay. Never
  execute asset generators or infer geometry from consumer footprints.
- Add `copper compile-macro SOURCE -o ASSET.json` for generated assets. Source
  bindings consume the same lowering directly, without intermediate user files.
- Update library discovery, LSP diagnostics/completion/navigation, VS Code
  highlighting, language/CLI references and a small runnable example.
- Verify equivalent source/JSON geometry, all schema features, repeated/nested
  bindings, portability, input changes, negative cases and editor overlays.

## Acceptance boundaries

Existing immutable ownership, exact pad/net bindings, via multiplicity,
protected regions, native physical checks and qualification gaps remain in
effect. Asset/schema validation does not qualify RF, switched power, assembly
or manufacturing. Changing source or footprint inputs invalidates old evidence.
No parameterized geometry, implicit mirroring, arcs, automatic routing,
automatic mappings or new library parts are introduced.

## Delivery and validation

Implemented: source AST/parser, exact quantity lowering, named package exports,
generated footprint identities, JSON compatibility, shared immutable binding,
source/footprint fingerprints, editor input tracking, library inspection, LSP
overlays/completion/navigation/diagnostics, highlighting, CLI and examples.
The normative syntax is in [macro-definitions.md](macro-definitions.md).

The selected macro/source/LSP/library/build/hard-macro/edge-opening/editor/guide
suite passed 322 tests with two optional/platform cases skipped. Additional
compiler/package/inventory/quantity checks plus definition tests passed 64 tests
with one host-dependent case skipped (the definition cases overlap the first
suite). The final macro/source/LSP run passed 100 tests after the portability
and output-input protection checks. Seven actual TextMate highlighting tests
passed. Commands used the
repository Python environment and disposable `build/macro-definition-tests*`
temporary directories.

Coverage includes equivalent source/generated-JSON binding and materialization,
all v0.5 authoring features, two providers, tag and commit revisions, local
replacements, unchanged locked inventory, source/footprint tampering, preparation
races, repeated module ownership, input revision invalidation, portable selection
serialization, missing/duplicate symbols and footprints, exact sub-nanometre
rejection, editor overlays and source-only route/export.

The included synthetic example compiled to JSON and completed `route-board`
with one immutable 2 mm track, zero vias, explicit-copper DRC pass and source
requirements pass. Native DRC/fill was not run for this example; no new hardware,
RF, assembly or manufacturing qualification is claimed.

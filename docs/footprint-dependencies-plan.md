# Footprint dependency implementation plan

Decision CS-150 extends the existing package/asset resolver. Footprint references
remain strings in electrical IR; geometry is resolved only for physical flows.

## Contract

- Accept GitHub/GitLab `host/owner/repository/path/File.kicad_mod` and its `https://`
  form in part defaults and component overrides. Select the repository revision
  from the consuming project's `require` entry, fetch once into `.copper-cache`,
  and verify the same complete `copper.lock` inventory used for reusable parts.
- Add `footprint-library NAME MODULE/DIRECTORY` in `copper.mod`. For example,
  `footprint-library Resistor_SMD gitlab.com/kicad/libraries/kicad-footprints/Resistor_SMD.pretty`
  resolves `Resistor_SMD:R_0402_1005Metric` without a machine-specific root.
  Require a matching module requirement. Duplicate or malformed bindings fail.
- Resolve relative `.kicad_mod` paths in imported parts and modules from their
  declaring package, confined to its module. Remote paths lower to portable
  module asset identities; workspace paths lower relative to the entry source.
- Exact URL references have one source. A namespace binding is authoritative
  unless the caller supplies an explicit matching local search root. An override
  must be recorded as local provenance and cannot satisfy a locked managed binding.
  Missing or invalid managed sources never fall through to another library.
- Retain existing explicit local files and legacy unbound dependency footprint
  lookup. Preserve missing/ambiguous/name/pad mismatch checks. Never auto-select
  a floating revision or discover arbitrary system installations.
- `copper lock BOARD` prepares every selected managed footprint dependency as
  well as source imports. It need not resolve unrelated local footprints.
  `--locked` requires and verifies the inventory without rewriting it;
  `--offline` forbids downloading missing modules, independently of locked mode.
- Audit, export, layout, routing and editor physicalization share one resolver.
  Reports retain the asset path, file SHA-256, module revision/inventory and
  namespace binding, or explicit local override provenance.

## Sequence and validation

1. Record CS-150 and user syntax in the specification and language reference.
2. Extend manifest parsing and shared asset resolution; add portable ownership
   qualification at imported part/module lowering.
3. Extend the footprint resolver and lock preparation. Exercise direct URLs,
   namespace bindings, package ownership and offline cached use through CLI
   audit/export. Keep locked source verification separate from local overrides.
4. Add regressions for tampered/missing locks, offline misses, malformed paths,
   duplicate bindings, filename/pad mismatches, namespace override policy and
   collisions between identically named package assets. Check physical flows
   share the behavior and cache verification does not repeat per component.
5. Add a runnable isolated example, document the flags and migration, run focused
   checks and the full suite, then integrate and push the verified change.

## Scope

GitHub and GitLab repositories use tagged/commit-pinned transport and complete
module inventories. Arbitrary web-file downloads, 3D-model URL rewriting and
automatic transitive dependency selection are separate work. The project declares
all footprint provider requirements. Local roots remain an explicit development
escape hatch, with their provenance and locked-mode limits visible.

## Progress

- CS-150, syntax, implementation, guide and standalone example completed.
- Focused package/footprint/CLI checks: 69 passed, one Windows symlink skip.
- Official KiCad 10.0.0 module downloaded and locked; namespace and exact URL
  footprints audited/exported with `--locked --offline` and no search roots.
  The lock stayed byte-identical; native KiCad 10 exported all requested layers.
- Full-suite inspection exercised 1,250 tests, including actual ngspice. It found
  legacy hard-macro identity compatibility (fixed; 49 affected checks passed,
  six optional skips) and five pre-existing routing-policy fixture failures
  reproduced on untouched `072105d` (fixture corrected; seven checks passed).
- Pinned v0.1 hard macros retain strict geometry hashes and module ownership
  while accepting their original module-root-relative footprint identity.
  Generated rigid members bind the current canonical asset identity/digest.
- Final full-suite rerun and integration/push follow these fixes.

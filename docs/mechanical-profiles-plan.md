# Mechanical profile implementation checklist

The [specification](mechanical-profiles.md) defines the implemented initial scope.
Checked items require passing evidence; no example geometry belongs in `pcbir`.

1. [x] Add root `board_profile`, profile-use AST, explicit role properties and
   instance names; preserve source locations and reject electrical declarations.
2. [x] Extend normal package exports/import contexts with profile definitions;
   support nested local/URL imports and profile composition without code execution.
3. [x] Add confined source-relative package imports and content provenance;
   retain remote inventory locks and source-module boundaries.
4. [x] Expand profiles with namespaced feature IDs, retained instance paths,
   role mapping and source ownership; reject cycles, duplicate roles/instances,
   missing/unknown bindings, competing outlines/rule fields.
5. [x] Lower geometry/keepouts to separate mechanical design, never electrical IR.
6. [x] Resolve connector anchors from unique physical pads, rotate/mirror offsets,
   enforce selected footprints and existing fixed/angle/side rules, freeze poses.
7. [x] Integrate JSON, physical placement/routing/DRC/KiCad and read-only editor
   role/provenance information. Preserve project-owned source patch boundaries.
8. [x] Add offline split-file example, negative/security/ownership regressions,
   real-footprint anchor/export checks and record verification/commit.

Follow-ups (not prerequisites for the above initial vertical):

9. [ ] Publish reviewed, revision-specific standard-board profiles in CopperLib,
   including Raspberry Pi variants once exact required boundary geometry exists.
10. [ ] Add exact rounded/arc outlines, mating axes/faces, height/host-interference
    contracts end-to-end; never claim unsupported standard compliance.
11. [ ] Extend editor persistence with imported-owner guards, exact diff review
    and validation before saving project-owned additions.
12. [ ] Consider typed profile parameters/transforms with explicit conflict and
    datum semantics; no text substitution or implicit overrides.

## Verification

- `pytest tests/test_mechanical_profiles.py -q -o addopts=''`: 56 tests pass.
  This includes profile/relative-import cycles, missing/unknown roles/components,
  conflicting outlines/rules/fixed poses/orientations, unique physical land
  requirements, import escape rejection, lock/hash changes, source-patch ownership,
  keepout serialization/export and preserved electrical hierarchy/connectivity.
  Additional probes retain imported poses through auto-placement and prove the
  routing clearance index blocks tracks/vias inside imported copper keepouts.
- Installed KiCad footprint and native `pcbnew.LoadBoard` probes independently
  verify anchor coordinates (within 1 nm) on front/back at 0°, 45° and 90°.
- Headless Chrome/Playwright profile smoke passes real CopperLib header, immutable
  imported pose, imported/local source-owner tooltips, three holes and both
  placement/copper keepout display. Zero page errors; no session/source mutations.
  Screenshot visually inspected at ignored `build/mechanical-profile-editor.png`.
- `copper check examples/mechanical_profile_project/board.copper --locked --offline`
  passes ERC using the pinned cached CopperLib; no manual library checkout.
- Real-footprint `export-kicad-pcb` writes the demo board/project/library into
  ignored `build/mechanical-profile/`. Explicit draft/omitted-text/3D warnings
  remain; this demonstration has no routing and is not production signoff.
- Final command: `pytest tests/test_mechanical_profiles.py
  tests/test_mechanical_language.py tests/test_packages.py tests/test_compiler.py
  tests/test_cli.py tests/test_layout.py tests/test_editor_source.py
  tests/test_mechanical_editor.py tests/test_internal_pad_connections.py
  tests/test_rigid_clusters.py tests/test_kicad_project.py -q -o addopts='' -rs`:
  **259 passed, 1 skipped**. The skip is the existing Windows symlink-creation
  permission probe; local-import confinement/guard regressions pass.
- Ordinary demo browser smoke also passes repeated drags, explicit previews,
  wheel zoom, right-pan, stale-revision recovery, locks and undo/redo after profile
  integration. Updated wheel contents match current IR/resolver/profile/UI files.
- Initial scope is complete. Follow-ups 9–12 are separate contracts: no standard
  HAT asset, use-site transforms/parameters, source saving, or mating/3D certification.

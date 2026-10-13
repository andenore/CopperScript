# Physical macro source syntax plan

Status: implemented source syntax and implementation record. This extends
the [current hard-macro contract](physical-hard-macros.md). It does not change
asset schemas, qualify existing geometry or add library definitions.

## Source syntax

Reuse the existing import shape for an exact macro asset, then bind it with a
typed physical declaration. Versions belong in `copper.mod`; generated content
hashes belong in `copper.lock` and build provenance. No hash is authored in the
binding declaration.

```copper
board Product {
    import regulator "github.com/vendor/power/packages/regulator";
    import buck_layout "github.com/vendor/power/packages/regulator/assets/layout.json";

    module PWR: regulator.Supply;

    // Electrical nets and supplies are declared normally.

    physical_macro BUCK: buck_layout on PWR {
        member U = U1;
        member L = L1;
        member CIN = C_IN;
        member COUT = C_OUT;

        net VIN = port.VIN;
        net VOUT = port.VOUT;
        net GND = port.GND;
        net SW = SWITCH;
    }
}
```

This is a schematic example: the selected asset must actually expose precisely
these member keys and net roles, and the selected electrical module must contain
the referenced components, ports and internal net.

```text
module example/product
require github.com/vendor/power v1.2.0
```

The URL denotes a file in the required module, following the existing managed
footprint convention. GitHub and GitLab module identities, with optional
`https://`, use the existing resolver. Revisions are supplied by `require`;
browser `blob`/`raw` URLs, embedded versions and arbitrary HTTP downloads are
not additional transport forms. A full commit remains a supported dependency
revision, but is not a required field in `.copper`.

Grammar additions:

```text
macro_import = "import", name, string, ";" ;
macro_binding = "physical_macro", name, ":", name,
                ["on", instance_path], "{", macro_mapping*, "}" ;
macro_mapping = "member", role_key, "=", instance_path, ";"
              | "net", role_key, "=", net_reference, ";" ;
role_key = name | string ;
```

The import syntax itself is unchanged. An exact `.json` import is classified
as a physical macro asset reference; a package-directory import retains current
source-package behavior. Physical preparation checks the supported macro schema;
an arbitrary JSON file is not a valid macro. This first version does not make
every JSON file in a package an exported symbol or add a new package manifest.

`physical_macro` is a dedicated declaration rather than a scalar constraint:
it has two typed mapping sets, an asset reference and optional instance scope.
`member` and `net` inside it are mappings, not component/net declarations.
Keys retain exact asset spelling; quoted keys support names outside the ordinary
identifier grammar. Right-hand references are checked identifiers, not strings.

## Binding scope and electrical authority

Without `on`, references resolve in the declaring board or module. A board can
bind an explicit component set spanning modules, preserving today's capability:

```copper
physical_macro MODEM_BUCK: buck_layout {
    member U = PWR/U_MODEM;
    member L = PWR/L_MODEM;
    // Bind every remaining asset member explicitly.
    net VIN = V5;
    net VOUT = V3V8;
    net GND = GND;
    net SW = PWR/MODEM_SW;
    // Bind every remaining asset net role explicitly.
}
```

`on PWR` makes right-hand references relative to that existing module instance.
It instantiates no electrical circuitry and grants no ownership of unrelated
components in that module. Nested module paths are allowed; references cannot
escape the selected subtree. Use the unscoped form for cross-module bindings.

Net references have explicit meanings:

- A net name resolves to an existing net in the selected scope.
- A descendant net path, such as `PWR/MODEM_SW`, resolves through hierarchy.
- A module port reference, such as `PWR.VIN` in board scope or `port.VIN` in
  module scope, selects its existing connected net.

Resolve all three through the electrical elaborator's actual net mapping.
Never prefix a boundary net blindly: a module-local VIN net may elaborate to
the board's V5 net. Port references do not create another connection mechanism.
Component-pin expressions are not net selectors in this first version.

Bindings are allowed in boards and reusable electrical modules, not parts,
devices or board profiles. Module-authored bindings are explicit requirements
of that module; repeated instances receive qualified macro identities and
independent owner copper. Boards normally select the realization with `on` so
the same electrical module can have different physical implementations.
Conflicting inherited and board bindings are errors, with both origins reported;
there is no implicit replacement or override syntax.

Member and net mappings remain explicit even when names match. No wildcard,
name guessing, positional mapping or automatic electrical rewiring is added.
Only the declared asset members are owned; other module components remain free.
Placement remains in ordinary constraints on member components. The asset's
anchor and relative poses define the rigid unit; no second pose system, implicit
mirroring or stackup adaptation is introduced.

## Dependency, local asset and lock behavior

Managed macro imports share the existing package cache, longest matching
requirement, `replace`, host-declared dependency policy and complete file
inventory. JSON is already an inventoried asset type; a second macro lockfile
or user-written SHA is unnecessary.

`copper lock board.copper` prepares macro imports reachable from the selected
design, validates asset schemas and mapping keys, and records/verifies their
module inventory alongside source and footprint dependencies. It does not
claim pad continuity or physical qualification without physical lowering.
Unreferenced package assets are not individually selected as macros.

`--locked` rejects changed or missing inventory entries and does not rewrite
the lock. It may refill missing cache content under existing resolver rules.
`--offline` prevents fetching; combining both reproduces from verified cache
or a matching local replacement. Missing requirements never acquire an implicit
version from the library's own manifest.

Local authoring uses the same import shape:

```copper
import buck_layout "./assets/buck-layout.json";
```

Relative imports belong to the declaring source file. Package-owned paths stay
inside their source module, even through nested imports and aliases; they never
fall back to the board directory. Entry-board local assets are workspace inputs,
like existing local footprints: capture their hashes automatically in physical
provenance and build fingerprints, but do not claim that `--locked` pins files
outside required modules. Use a required module with a local `replace` when
package-lock enforcement is needed during local development. Absolute local
macro import paths are outside the first version.

At physical preparation, retain the verified asset identity, version, module
inventory checksum and per-file SHA from the resolver. Supply that generated
expected digest to the existing binder and verify the bytes again when binding.
Do not replace identity verification with an unchecked hash of whatever file
happens to be present. Local input snapshots likewise must detect changes
between preparation, binding and replay. Asset changes invalidate old evidence.

## Compiler and command behavior

Parsing produces source-located typed macro import and mapping records. Semantic
checking validates alias kinds, scopes and component/net references. Electrical
compilation remains usable without downloading macro geometry, like footprint
geometry today; report asset-schema, mapping-key and physical checks as pending
until their inputs are prepared. Compilation never reports those checks passed
merely because the references are syntactically valid.

Retain realization descriptors alongside authoritative electrical IR on `Design`,
including module-owned templates and their source ownership. The electrical
netlist stays unchanged. Serialization must retain these descriptors so a
compiled design cannot silently lose macros. Physical elaboration resolves
scopes and boundary nets, then lowers through `bind_hard_macro()` and the
existing transactional materialization and validation lifecycle.

All physical entry points consume the same resolved declarations: placement,
planning, `route-board`, direct PCB/project export and editor rebuilds. A route
rebuild starts from the same bound source and accepted unit pose. Repeated
loading must not append duplicate macro records.

Diagnostics must distinguish unknown aliases/members/nets, duplicate mapping
keys, incomplete asset coverage, unsupported schemas, lock mismatches, footprint
or pad/net mismatches, conflicting ownership, and unsupported stackup/pose
contracts. Attach errors to the relevant mapping or import and identify the asset.
Duplicate aliases and binding names are errors; bindings may not reuse an owned
component or collapse distinct asset net roles onto one physical net.

Keep the existing JSON scene and `--hard-macro` interface compatible, including
its explicit `asset_sha256` checks. Source declarations and scenes can coexist
only with distinct identities and disjoint member ownership. Reject conflicts;
neither input silently wins. The CLI option becomes unnecessary for source-bound
macros. No immediate deprecation or bulk consumer migration is required.

## Implementation sequence and acceptance

Implementation checklist:

- [x] Parser and source-located typed import/binding records.
- [x] Reference checking, module scopes, repeated/nested instance expansion and
  parent boundary-net aliases without electrical rewiring.
- [x] Existing URL resolver/lock preparation, relative source ownership and
  generated asset digests; no source SHA field or additional lock schema.
- [x] Serialized realization descriptors and shared physical lowering through
  the existing immutable binder; source/scene ownership conflict checks.
- [x] Editor input tracking and diagnostic-offset-independent intent identity.
- [x] Locked/offline language-server contracts, mapping-key/scoped-target
  completion, diagnostics and navigation; library requirement inspection.
- [x] Source requirement rows for missing/changed assets and immutable owners,
  retaining separate electrical/RF/power/native/manufacturing qualification.
- [x] Synthetic consumer regressions and current language/CLI/agent references.

Realization descriptors are retained on board/module records, exposed through
`Design.electrical`, with geometry exclusively in assets and physical IR. This
keeps the existing electrical projection and JSON serializer from dropping
bindings. Empty realization fields are omitted from serialized designs to
preserve existing output. Electrical connectivity remains unchanged.

1. Add parser/AST records, typed alias handling and reference diagnostics.
   Preserve all existing package import behavior and source locations.
2. Preserve realization descriptors in `Design` and serialized designs. Add
   hierarchy expansion using actual elaborated component and net identities,
   including repeated/nested instances and parent-connected module ports.
3. Add macro asset preparation through `PackageResolver.resolve_asset()` and
   declaring-source path qualification; extend `copper lock` preparation.
   Reuse verified inventory digests without changing existing package pins.
4. Connect shared physical lowering and every physical command/editor rebuild
   to the existing binder, routing lifecycle, fingerprints and evidence origins.
   Preserve legacy scene support and reject mixed-source ownership conflicts.
5. Add language-server diagnostics, member/net completion from cached contracts,
   references and navigation. Inspection stays locked/offline and displays
   missing asset contracts; it does not fetch or update locks implicitly.
6. Document implemented syntax in the language/macro/CLI references and migrate
   one small synthetic consumer fixture to demonstrate URL and local usage.
   Do not expand CopperLib or change real board assets for this compiler work.

Required regression coverage:

- Two independent asset providers; tag/commit requirements, `replace`, cache
  reuse, locked/offline operation, missing providers and tampered JSON rejection.
- Local and package-relative asset ownership, nested source imports, traversal
  and symlink rejection, and changes between preparation and binding.
- Scoped/unscoped mappings, forward references, quoted keys, unknown/duplicate
  names, incomplete coverage, repeated instances and boundary-net coalescing.
- Wrong packages/pads/nets, isolated lands, unsupported schemas/stackups,
  contradictory poses and duplicate source/scene ownership fail visibly.
- Equivalent source and legacy scene bindings produce equivalent physical
  geometry and owner contracts; provenance includes the declaration origin.
- Placement, bounded routing, feedback, export and editor rebuilds retain
  immutable tracks/vias/polygons/zones, required via-array multiplicity, protected
  regions and proved package/boundary exits, including on partial nets.
- Electrical nets are unchanged by binding selection; unprepared/unsupported
  verifiers stay visible; serialization and replay cannot omit declarations.

Named exported realization templates, automatic member/net binding, default
variant selection, override syntax, mirrored geometry and new RF/power verifiers
are separate future work. The first implementation changes how existing macros
are declared and resolved, while retaining their physical and qualification
contracts.

## Validation record

The new `tests/test_macro_source.py` suite has 37 passing cases. Together with
the language-server suite, the final feature/editor run passed 70 tests. It
includes source-only `route-board`, `plan-layout` and PCB export, a partial net
with external routing, two asset providers, scoped/nested/repeated bindings,
lock tampering, preparation/binding races, missing/changed owner requirements,
and source editor rebuild/input tracking.

The broader selected compiler, package, macro, library, physical IR, rigid
cluster, editor, requirement, agent-guide, via-array, partial-package and
boundary-routing regressions passed 396 tests, with two optional cases skipped.
Twelve editor HTTP cases initially hit sandbox loopback restrictions; they all
passed when rerun with loopback access. One installed-KiCad boundary regression
initially hit the sandbox temporary-directory restriction; it passed with its
temporary files placed inside the workspace. These environment failures were
resolved without changing tests or weakening checks.

Use the repository's Python environment and a fresh workspace `--basetemp` when
reproducing these checks. The compact final feature command is:

```powershell
.\.venv\Scripts\python.exe -m pytest tests/test_macro_source.py tests/test_lsp.py -o addopts= -q --basetemp build/macro-feature-checks
```

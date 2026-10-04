# Footprint dependencies

A reusable part can declare its footprint once. Physicalization, audit, layout,
routing and the mechanical editor use the same resolver. Geometry is fetched
from versioned dependencies when needed, and then reused from the managed cache.
Electrical compilation and simulation do not require footprint geometry.

## KiCad library identifiers

Declare the footprint in the part:

```copper
part R0402 {
    category = "passive.resistor";
    footprint = "Resistor_SMD:R_0402_1005Metric";
    pin A { number = "1"; domains = "analog"; directions = "passive"; }
    pin B { number = "2"; domains = "analog"; directions = "passive"; }
}
```

Bind that namespace in the consuming project's `copper.mod`:

```text
module example/my-board
require gitlab.com/kicad/libraries/kicad-footprints eff413fd489b7ade27fc108781a95c4b28ccf0b7
footprint-library Resistor_SMD gitlab.com/kicad/libraries/kicad-footprints/Resistor_SMD.pretty
```

That revision is the official KiCad 10.0.0 footprint release commit. The binding
selects exactly `Resistor_SMD.pretty/R_0402_1005Metric.kicad_mod` inside that
revision. Repeat `footprint-library` for each namespace needed by the project;
all bindings to the same module share its download and inventory. A binding may
also select the module root. Duplicate namespace bindings, malformed paths and
bindings without matching `require` entries are errors.

Current official KiCad libraries are hosted on
[GitLab](https://gitlab.com/kicad/libraries/kicad-footprints). The former
[GitHub repository](https://github.com/KiCad/kicad-footprints) is archived.
Both hosts are supported by the package transport; GitLab subgroup paths are
supported. Choose an explicit published tag or full commit SHA in `require`.
The repository itself does not need to contain CopperScript source files.

## Exact module asset URLs

Instead of a namespace binding, set an exact asset path in a part or component:

```copper
footprint = "https://gitlab.com/kicad/libraries/kicad-footprints/Resistor_SMD.pretty/R_0402_1005Metric.kicad_mod";
```

The equivalent path without `https://` also works. The project's `require`
entry still supplies the revision. These are module identities, not browser
`blob`, `raw` or arbitrary web-download links: omit query strings, fragments,
percent encoding and embedded versions. Exact URLs always resolve the managed
asset and never search local roots. Paths must remain inside the module.

## Footprints bundled with parts

Inside an imported package, a relative `.kicad_mod` path belongs to the file
that declares it:

```copper
footprint = "footprints/MyConnector.kicad_mod";
```

The same rule applies to component overrides authored inside reusable modules.
Relative paths may reach another directory within the source module, such as
`../footprints/MyConnector.kicad_mod`; escaping the source module is rejected.
The compiler lowers these paths to portable module asset identities for external
dependencies, or entry-source-relative paths for workspace imports. Aliases and
nested instances preserve ownership. Identically named assets in two packages
stay distinct and cannot bind accidentally to a board-local file. Existing pinned
hard macros may retain a module-root-relative asset identity: binding verifies
that the footprint and macro belong to the same module and that the original
geometry digest still matches, then retains the canonical identity internally.

Relative `.kicad_mod` overrides written in the entry board retain their existing
board-source-directory behavior. Absolute local files are supported there.
Package-owned footprint paths must be relative, with forward slashes.
The project explicitly declares all external provider requirements; a library's
own manifest does not silently add or select a provider version.

## Preparing and reproducing a build

```text
copper lock board.copper
copper audit-footprints board.copper --locked --offline --json
copper export-kicad-pcb board.copper --locked --offline -o build/board.kicad_pcb
```

`lock` prepares selected managed footprint dependencies as well as imported
source packages, and creates/updates `copper.lock`. This inventories complete
module content, including individual footprint SHA-256 hashes. Downloaded
modules live in the project-local `.copper-cache/pkg` cache. The first geometry
resolution downloads a complete module; subsequent builds reuse it.

| Flag | Behavior |
| --- | --- |
| `--locked` | Require the declared dependency revision and complete file inventory to match `copper.lock`. Never rewrite the lock. Downloads remain allowed to refill missing cache entries. |
| `--offline` | Prevent remote fetching. Required modules must already be cached or supplied through a local replacement. An unlocked offline operation may still create/update a lock. |

A lockfile alone does not contain the geometry. Prepare the cache before using
both flags. The editor already uses both flags, so prepare the project with
`copper lock` before opening an uncached managed-footprint design there.

`copper check`/`compile` resolve imported source dependencies but do not download
geometry merely because a part declares a footprint. `copper lock` prepares only
selected managed references; it does not require unrelated local footprint files.
Physical commands still validate declared footprint names, numbered pad coverage
and supported geometry, whether the source is managed or local.

## Local overrides and provenance

Existing local files, explicit `--footprint-root` search directories and legacy
dependency `footprints/` lookup remain supported. A matching explicit local root
can override a manifest-bound namespace during an unlocked development operation.
Multiple matching roots are an error. In locked mode a bound namespace always
uses its verified managed provider, even if a matching local root was supplied.
An invalid/missing managed provider never falls through to a different library.

Footprint metadata and audit JSON retain the selected file path/SHA-256 and
resolution route. Managed assets also identify the module, revision, inventory
checksum and asset identity; namespace references identify their binding. Local
overrides are labeled `local_override`. Files outside required modules are
local inputs and are not pinned by the package lock. No system KiCad installation
is discovered implicitly.

See the [runnable project](../examples/managed_footprints/README.md),
[specification decision CS-150](design-specification.md#cs-150--footprints-as-resolvable-package-dependencies-accepted)
and [implementation plan](footprint-dependencies-plan.md).

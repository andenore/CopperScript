# Browser compiler boundary

`analyze` and `review` retain the standalone playground contract (32 KiB; no
imports). `review_project` accepts a JSON source snapshot, discovers imports with
the ordinary parser/package resolver, and compiles a complete snapshot with the
ordinary compiler, serializer, ERC and queries. It does not access network, Git,
caches or lockfiles on disk. Hosts supply requested files without executing code.

Input `copperscript.project-snapshot/v1` contains `entry` (workspace-relative board
path), `manifest` / `lock` (original UTF-8 text), `workspace_files` (regular `.copper`
paths) and `files` (source ID to original UTF-8 text). Source IDs are
`workspace/PATH` or `dependency/MODULE/PATH`. Preserve CRLF/BOM bytes during decoding:
source checksums are verified before the compiler's newline normalization.

An incomplete snapshot returns `copperscript.project-plan/v1` with `requests`.
Each request has `id`, `path`, and, for external source, `repository`, `revision`,
`version`, `module`, `size`, and `sha256`. Download at a fixed repository commit and
resubmit the enlarged snapshot. Only declared lock inventory entries are eligible.
The browser host currently supports public GitHub modules. Local imports stay
within the workspace or their owning dependency module. Local replacements must
stay inside the workspace. Missing lock pins and source/inventory mismatches fail;
there is no preparation or lock update fallback. Native disk resolution is unchanged.

A complete snapshot returns `copperscript.review-analysis/v1`, including exact
native electrical IR text and `source_snapshot` provenance for the loaded files.
Syntax, package and compilation failures return diagnostics without IR. Cycles
remain ordinary compiler errors. ERC findings with a compilable design stay
reviewable. Root diagnostics use virtual source paths, linked to original source
IDs by snapshot provenance.

This boundary verifies **loaded electrical sources**. It validates the declared
lock inventory checksum but does not fetch/verify physical assets or assert a full
module checkout match. Workspace dependency checksums cover loaded source files.
Physical macro bindings are retained and checked by normal compilation; macro
asset geometry, footprint/3D content, layout and manufacturing qualification are
outside this boundary. Report this scope in a consumer's UI and exported provenance.

Limits: 512 KiB per source, 8 MiB loaded source, 512 loaded files, 8,192 known source
paths, 24 MiB snapshot JSON. Hosts must run analysis in a replaceable bounded worker
and bound downloads/expansion too; CopperReview uses 15 seconds per analysis pass,
120 seconds per project, 32 expansion passes and six concurrent downloads.

Run `cargo test --locked -p copper-lang -p copper-web --lib` and the consumer's
actual WASM tests. `copper-lang::snapshot` tests cover transitive/relative imports,
cycles, containment and lock-byte checks. Native CLI/package behavior stays on its
existing resolver; it does not opt into snapshots implicitly.

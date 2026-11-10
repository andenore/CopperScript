# Shared Make builds

Every board uses the same recipes in `make/board.mk`. Board settings live in
`make/examples/*.mk`; electrical/mechanical intent and physical macro bindings
remain in `.copper`, while macro geometry and placement scenes remain data files. There are no
per-board routing wrappers. CI (the `make-targets` job in
`.github/workflows/rust.yml`) uses these same Make configurations.

Requires GNU Make, the `copper` executable on PATH (installed with
`cargo install --locked --path crates/copper`, or a release binary), and KiCad 10
plus its footprints for physical targets. The `route` target also needs bash,
`jq`, `sha256sum` (or `shasum`) and KiCad's `kicad-cli`. `make/board.mk` sets
`COPPER ?= copper`; override `COPPER` with another executable path when needed.
On Windows GNU Make is an additional tool, not bundled with CopperScript;
`mingw32-make` can be used wherever commands below say `make`.

## From the repository root

```sh
make EXAMPLE=cm4 check
make EXAMPLE=cm4 compile
make EXAMPLE=cm4 route
make EXAMPLE=full-vertical route
make EXAMPLE=nrf52 route
make compile-examples
make check-examples
```

Targets:

| Target | Result |
| --- | --- |
| `all` (default) | ERC and JSON compilation; no expensive routing |
| `check` | ERC |
| `compile` | Hierarchical electrical/mechanical JSON IR |
| `layout` | Placement draft and report, not a routed board |
| `edit` | Mechanical/floorplan editor |
| `route` | Rust package escape/routing, saved copper fill and native DRC |
| `release` | Checksummed bundle of a board's order export, its route run and BOM for manual archiving (see [Release bundles](#release-bundles)) |
| `compile-examples` (root only) | Compile the registered examples and compatibility fixtures |
| `check-examples` (root only) | Run `check`, `compile`, and a dry-run of `route` for every registered complete example |

`compile-examples` and `check-examples` exclude `examples/invalid_board/board.copper` and the intentionally
incomplete `examples/nrf_antenna_macro/board.copper` probe. It does not waive ERC. A selectable
example is not a promise of successful routing or production readiness: only
an actual passed route/fill/DRC run establishes that board's connectivity.
The CM4 four-layer routing preset has been verified with KiCad 10.0.6.

For the LED-ring board, use [CopperLedRing](https://github.com/andenore/CopperLedRing)
and its own pinned build targets. `EXAMPLE=round-led-ring` is deprecated; it
remains selectable and in the fast regression checks for compatibility.
Its legacy geometry tests do not establish the standalone project's routing
status. See the [deprecation notes](round-led-ring.md).

`check-examples` checks the fast targets and expands every route recipe without starting
routing. CI additionally runs the `layout` and non-interactive `edit` targets for the
physical examples and compatibility fixtures (`cm4`, `full-vertical`, `nrf52`,
the deprecated `round-led-ring`, the mechanical
examples, and `resolved-footprint`) on each push. Electrical and simulation fixtures
without complete footprint identities still run their complete fast target set. Actual
routing runs in the separate `ladder` job (`copper bench bench/ladder.json`).

Compilation/placement outputs live under ignored `build/<board-name>/`.
Each route gets a fresh `build/<board-name>/runs/<UTC-id>/`; open its
`board.kicad_pcb` alongside its generated `.kicad_pro`, `fp-lib-table` and local
`.pretty` library. The directory also contains the route/native reports, `routing.log` and
`run.json` with the input and output hashes. No recursive clean target deletes
past evidence. Set `RUN_DIR` to a **new/empty path below project `build/`** if a
fixed CI destination is needed.

The `route` target runs `bash make/route-build.sh`. It runs `copper check
--macro-inputs` to resolve this board's imports and physical macro assets
through its nearest manifest (fetching only pinned URL dependencies; `run.json`
records the SHA-256 of every macro input the board binds), then routes offline:

1. `copper route-board SOURCE ... --offline --verify-plane-fill KICAD_CLI
   --report RUN/route-report.json -o RUN/board.kicad_pcb`;
2. `kicad-cli pcb drc --refill-zones --save-board` on the routed board, which
   saves the filled copper in the deliverable and writes `kicad-drc.json`;
3. a `jq` completion check: success needs report status `pass`, ERC pass,
   complete routing, native fill verification, no unrouted ordinary nets, and
   zero native violations and unconnected items;
4. `run.json` with commands, provenance and the SHA-256 bindings
   `editor_intent_sha256`, `draft_board_sha256`, `filled_board_sha256` and
   `native_drc_sha256` that `copper editor-overlay` checks.

It never assumes CopperLib or a sibling checkout and preserves routing errors.
Exit status: 0 passed, 1 unmet completion gates, 2 setup or runner error, 3
unsupported (the compiler's not-implemented exit), 130 interrupted.
The shared default chooses among the placement planner's actual legal candidates;
it does not force `candidate-00`, which may have been rejected. To reproduce an
explicit candidate, set `EXTRA_ROUTE_ARGS="--placement-candidate candidate-01"`;
an unavailable requested identity remains an error rather than a silent fallback.
GNU Make normally returns 2 when a recipe fails; inspect `run.json` for the
underlying routing exit code. No failure is suppressed with `-` or `|| true`.

## Overrides

KiCad paths are automatically discovered for `route` from environment/PATH or
the standard Windows KiCad 10 installation. On Linux, `layout` and `edit` use
`/usr/share/kicad/footprints` by default. Set `KICAD_FOOTPRINTS` for a custom
or nonstandard KiCad installation; physical settings and explicit scenes are
shared across targets.

```powershell
make EXAMPLE=cm4 route KICAD_CLI="C:/Program Files/KiCad/10.0/bin/kicad-cli.exe" KICAD_FOOTPRINTS="C:/Program Files/KiCad/10.0/share/kicad/footprints"
make EXAMPLE=cm4 layout KICAD_FOOTPRINTS="C:/Program Files/KiCad/10.0/share/kicad/footprints"
make EXAMPLE=cm4 route RESOLVE_ARGS="--locked --offline"
make EXAMPLE=cm4 route BUILD_ARGS=--dry-run
make EXAMPLE=cm4 route COPPER=/path/to/rust/copper
make EXAMPLE=full-vertical route EXTRA_ROUTE_ARGS="--routing-profile thorough"
```

`make -n EXAMPLE=cm4 route` prints recipes without executing them. `BUILD_ARGS=--dry-run`
runs setup/command validation against cached dependencies without writing output.
`PHYSICAL_ARGS`, `ROUTE_ARGS` and `EXTRA_ROUTE_ARGS` accept normal CLI options;
the shared runner controls output paths, report paths and final fill verification.
Profiles are preferences, never hardcoded geometry or weakened DRC rules.

## Another source or project

Any `.copper` board can use the root recipes, not just registered examples:

```sh
make SOURCE=examples/hierarchical_board/board.copper compile
make SOURCE=path/to/my_board.copper route FAB_PROFILE=jlcpcb-four-layer
```

Use the default `EXAMPLE=valid` when overriding `SOURCE` to avoid carrying
another example's optional physical scenes. For an independent repository,
include or copy the shared `make/board.mk` and `make/route-build.sh`, install
`copper` and use a
settings-only Makefile such as:

```make
SOURCE := board.copper
FAB_PROFILE := jlcpcb-four-layer
include make/board.mk
```

Layer selection comes from the board's declared stackup and layer requirements,
with fabrication presets supplying a compatible count when needed. The shared
recipes leave `LAYERS` empty by default. `make LAYERS=4 ...` remains an explicit
check/selection and cannot replace a conflicting source stackup. Use a compiler
containing the source-configuration change (`c70ac72` or later). Physical
stackups, planes and net rules belong in `.copper`; fabrication-preset selection
and solver budgets remain build settings until their source syntax is supported.

The independent project owns its manifest/lock and `build/` gitignore entry;
it needs no example-specific script. `COPPER` selects the executable, and
`make/route-build.sh` must be next to `board.mk`. Unsupported features fail
explicitly (exit 3). `KICAD_PYTHON` selects KiCad's own Python for
`make editor-overlay` (`copper editor-overlay --kicad-python`). In this checkout the CM4 local
Makefile also includes the shared recipes: `make -C examples/cm4_baseboard route`.

Successful routing is **not manufacturing signoff**. Power/current review,
mechanical/RF qualification, exact assembly selections and manufacturing export
remain separate obligations; this workflow does not create Gerbers or waive DRC.

## Release bundles

`make release RELEASE=<part>-<rev>` collects what was ordered into
`build/release/<part>-<rev>/` and a zip of it, for archiving by hand. It runs
`make/release-build.sh` after the project's `bom` target (clear `RELEASE_DEPS`
for a board without one) and reads the project's `ORDER` and `LOCK` settings.

| Bundle path | Content |
| --- | --- |
| `fab/` | The manufacturing export under `ORDER` (the one directory with a `manifest.json`; `RELEASE_FAB` names another) |
| `route/` | `run.json`, DRC, route summary, logs and via process of the route run whose filled board was exported |
| `bom/` | `ORDER`'s CSV lists and assembly report, and the lock |
| `orders/` | Empty, for the order confirmations |
| `RELEASE.md` | Commit, remote, dependency pins, KiCad version, route run and the manifest's qualification flags, with a Notes section to fill in |
| `SHA256SUMS` | Every file in the bundle |

It refuses uncommitted tracked changes, an existing bundle, fab files that no
longer match their `SHA256SUMS` (the `.kicad_pro`/`.kicad_prl` settings, which
KiCad rewrites on opening, are exempt), and an export without a passed route
run. The route run is found by the exported board's SHA-256. If a tracked input
recorded in that run (the source, `copper.mod`, `copper.lock`, the Makefile)
differs from `HEAD`, the build stops; set `RELEASE_ACCEPT_DRIFT=1` only when the
changes leave the fabricated board unchanged, such as BOM-only value changes,
and RELEASE.md then lists them. Tagging, pushing and uploading the bundle stay
manual; the target prints the commands.

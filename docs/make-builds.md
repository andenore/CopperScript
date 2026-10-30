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

The `route` target runs `bash make/route-build.sh`. It runs `copper check` to
resolve this board's imports through its nearest manifest (fetching only pinned
URL dependencies), then routes offline:

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
make EXAMPLE=full-vertical route EXTRA_ROUTE_ARGS="--zone-dependency-expansions 0"
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
make SOURCE=path/to/my_board.copper route LAYERS=4 FAB_PROFILE=jlcpcb-four-layer
```

Use the default `EXAMPLE=valid` when overriding `SOURCE` to avoid carrying
another example's optional physical scenes. For an independent repository,
include or copy the shared `make/board.mk` and `make/route-build.sh`, install
`copper` and use a
settings-only Makefile such as:

```make
SOURCE := board.copper
LAYERS := 4
FAB_PROFILE := jlcpcb-four-layer
include make/board.mk
```

The independent project owns its manifest/lock and `build/` gitignore entry;
it needs no example-specific script. `COPPER` selects the executable, and
`make/route-build.sh` must be next to `board.mk`. Unsupported features fail
explicitly (exit 3). `KICAD_PYTHON` selects KiCad's own Python for
`make editor-overlay` (`copper editor-overlay --kicad-python`). In this checkout the CM4 local
Makefile also includes the shared recipes: `make -C examples/cm4_baseboard route`.

Successful routing is **not manufacturing signoff**. Power/current review,
mechanical/RF qualification, exact assembly selections and manufacturing export
remain separate obligations; this workflow does not create Gerbers or waive DRC.

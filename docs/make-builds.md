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
| `route` | `copper build`: package escape/routing, saved copper fill and DRC (CopperScript's own; KiCad's with `KICAD_CLI`) |
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
`.pretty` library. The directory also contains the route and DRC reports, `routing.log` and
`run.json` with the input and output hashes. No recursive clean target deletes
past evidence. Set `RUN_DIR` to a **new/empty path below project `build/`** if a
fixed CI destination is needed.

The `route` target runs [`copper build`](#copper-build-the-same-build-without-make).
With `KICAD_CLI` empty (the default) it refills and checks the board with
CopperScript itself and needs no KiCad; `make route KICAD_CLI=/path/to/kicad-cli`
selects KiCad's refill and DRC (`--verify kicad`). It runs `copper check
--macro-inputs` to resolve this board's imports and physical macro assets
through its nearest manifest (fetching only pinned URL dependencies; `run.json`
records the SHA-256 of every macro input the board binds), then routes offline:

1. `copper route-board SOURCE ... --offline --verify-plane-fill native
   --report RUN/route-report.json -o RUN/board.kicad_pcb` (the kicad-cli path
   instead of `native` with `KICAD_CLI`);
2. a refill of the routed board saved in the deliverable, and a DRC of it:
   CopperScript's filler and DRC writing `native-drc.json`, or `kicad-cli pcb
   drc --refill-zones --save-board` writing `kicad-drc.json`;
3. a completion check: success needs report status `pass`, ERC pass,
   complete routing, fill verification, no unrouted ordinary nets, and
   zero DRC violations and unconnected items;
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

## `copper build`: the same build without Make

`copper build SOURCE [options] -- [ROUTE-BOARD OPTIONS...]` is what `make route`
runs (it replaced the bash script `make/route-build.sh`): it needs neither GNU
Make, bash, `jq`, `sha256sum` nor KiCad, and runs `check`, `route-board`, the refill and DRC in
the `copper` binary. It prints its step plan first (`--dry-run` prints it,
checks the source offline and writes nothing), uses the same options
(`--project-root`, `--output-root`, `--output-dir`, `--kicad-cli`,
`--kicad-footprints`, `--locked`, `--offline`), the same run directory layout,
the same refusal of an existing non-empty directory and the same exit status
(0, 1, 2, 3). `--copper` does not exist, because the compiler is the running
binary.

```sh
copper build board.copper --locked
copper build board.copper --locked -- --layers 4 --routing-profile balanced
copper build board.copper --verify kicad --kicad-cli /path/to/kicad-cli -- --layers 4
```

`route-board` takes its defaults from the board's
[`build` block](language-reference.md#board-build-settings): fabrication
preset, placement candidates, routing effort and the other settings listed
there. An option after `--` overrides the source. The plan shows the options
taken from the source, and `run.json` records them as `source_build_options`
next to `routing_command`. The build always passes `--progress` to
`route-board`.

`--verify native` (the default) refills the routed board with CopperScript's
filler and checks it with its DRC, writing `native-drc.json`.
`--verify kicad` runs `kicad-cli pcb drc --refill-zones --save-board` exactly as
the script does and writes `kicad-drc.json`; use it when KiCad's DRC is the
check you want (it also covers courtyards and silkscreen) and to cross-check a
native result. `run.json` records the mode (`verify`) and, besides the
script's fields, a `schema` and the name of the DRC report (`drc_report`).
`copper editor-overlay` reads the report `drc_report` names (`kicad-drc.json` when
the manifest has none, as the script's runs do), so it accepts either mode.
`make/release-build.sh` bundles the report the run names.

A native build passes a footprint root to `route-board` only when you give
`--kicad-footprints` (or `--footprint-root` after `--`), so it resolves
footprints the same way on every machine, whether or not KiCad is installed.
Boards that use KiCad footprint ids therefore need one of the two; boards whose
footprints come from pinned packages need neither. A `--kicad-cli` or
`--kicad-footprints` path that does not exist is an error in both modes.

`make route` only adds the shared variables of `make/board.mk`; the run
directory is the same either way. Press Ctrl-C on a `copper build` and the
process ends at once, leaving `run.json` with status `running`.

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
`ROUTE_ARGS` defaults to `--candidates $(ROUTE_CANDIDATES)`; the other route
options use the compiler defaults or the board's `build` block. `FAB_PROFILE`
is empty by default, which uses `fab_profile` of the `build` block, else
`generic`. A Make variable is a command line option, so it overrides the source.
Profiles are preferences, never hardcoded geometry or weakened DRC rules.

## Another source or project

Any `.copper` board can use the root recipes, not just registered examples:

```sh
make SOURCE=examples/hierarchical_board/board.copper compile
make SOURCE=path/to/my_board.copper route FAB_PROFILE=jlcpcb-four-layer
```

Use the default `EXAMPLE=valid` when overriding `SOURCE` to avoid carrying
another example's optional physical scenes.

An independent project does not include or copy `make/board.mk`. It keeps one
Makefile of its own in which every target is a plain `copper` call, so the
command a target runs can be read, copied and run by hand. The build settings
are in the board source, not in the Makefile:

```copper
build {
    fab_profile = "jlcpcb-four-layer";
    placement { candidates = 1; }
}
```

```make
COPPER ?= copper
# Every route is a new build/board/<UTC time>-<pid>/ with a run.json; RUN is the newest.
RUN = $(patsubst %/run.json,%,$(lastword $(sort $(wildcard build/board/*/run.json))))

check:
	$(COPPER) check board.copper --locked
route:
	$(COPPER) build board.copper --locked
bom:
	$(COPPER) assembly check board.copper --locked --offline --lock assembly.lock
	$(COPPER) assembly bom board.copper --locked --offline --lock assembly.lock -o build/order/bom.csv
order: bom
	$(COPPER) export-manufacturing $(RUN)/board.kicad_pcb --run $(RUN)/run.json --skip-independent-cam --bom build/order/bom.csv --replace -o build/order/jlcpcb
```

`--run` makes the export refuse a route that did not pass or that predates a
source change, so the Makefile needs no checks of its own. The rest of this
page describes the recipes this repository uses for its own examples.

Layer selection comes from the board's declared stackup and layer requirements,
with fabrication presets supplying a compatible count when needed. The shared
recipes leave `LAYERS` empty by default. `make LAYERS=4 ...` remains an explicit
check/selection and cannot replace a conflicting source stackup. Use a compiler
containing the source-configuration change (`c70ac72` or later). Physical
stackups, planes and net rules belong in `.copper`; fabrication-preset selection
and solver budgets remain build settings until their source syntax is supported.

The independent project owns its manifest/lock and `build/` gitignore entry;
it needs no example-specific script. `COPPER` selects the executable. Unsupported features fail
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

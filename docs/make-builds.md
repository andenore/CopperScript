# Shared Make builds

Every board uses the same recipes in `make/board.mk`. Board settings live in
`make/examples/*.mk`; electrical/mechanical intent remains in `.copper`, and
explicit hard-macro/placement scenes remain data files. There are no CM4 or
full-vertical Python routing wrappers. CI uses these same Make configurations;
it explicitly sets `PLACEMENT_TEMPLATES=` for its full-vertical run to preserve
its source-only placement policy, and owns timeout/artifact handling separately.

Requires GNU Make, an installed CopperScript environment (`uv sync --extra test`
from this checkout), and KiCad 10 plus its footprints for physical targets.
On Windows GNU Make is an additional tool, not bundled with CopperScript;
`mingw32-make` can be used wherever commands below say `make`.

## From the repository root

```sh
make EXAMPLE=cm4 check
make EXAMPLE=cm4 compile
make EXAMPLE=cm4 route
make EXAMPLE=full-vertical route
make EXAMPLE=nrf52 route
make EXAMPLE=round-led-ring route
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
| `route` | Profiled package escape/routing, saved copper fill and native DRC |
| `compile-examples` (root only) | Compile all 16 registered complete examples |
| `check-examples` (root only) | Run `check`, `compile`, and a dry-run of `route` for every registered complete example |

`compile-examples` and `check-examples` exclude `examples/invalid_board/board.copper` and the intentionally
incomplete `examples/nrf_antenna_macro/board.copper` probe. It does not waive ERC. A selectable
example is not a promise of successful routing or production readiness: only
an actual passed route/fill/DRC run establishes that board's connectivity.
The CM4 four-layer routing preset has been verified with KiCad 10.0.6.

`check-examples` checks the fast targets and expands every route recipe without starting
routing. CI additionally runs the `layout` and non-interactive `edit` targets for every
registered example on each commit. Actual routing remains limited to release tags.

Compilation/placement outputs live under ignored `build/<board-name>/`.
Each route gets a fresh `build/<board-name>/runs/<UTC-id>/`; open its
`board.kicad_pcb` alongside its generated `.kicad_pro`, `fp-lib-table` and local
`.pretty` library. The directory also contains the route/native reports, logs,
profiling summaries, hashes and `run.json`. No recursive clean target deletes
past evidence. Set `RUN_DIR` to a **new/empty path below project `build/`** if a
fixed CI destination is needed.

The generic `pcbir.build` runner resolves this board's imports through its
nearest manifest, fetches only pinned URL dependencies, then routes offline.
It never assumes CopperLib or a sibling checkout. It preserves routing errors,
saves filled copper in the deliverable and accepts success only with ERC pass,
complete routing, native fill verification, zero violations of any severity
and zero unconnected items. Profiling does not change exit semantics.
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
make EXAMPLE=cm4 route PROFILE=none
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
include or copy the shared `make/board.mk`, install CopperScript and use a
settings-only Makefile such as:

```make
SOURCE := board.copper
LAYERS := 4
FAB_PROFILE := jlcpcb-four-layer
include make/board.mk
```

The independent project owns its manifest/lock and `build/` gitignore entry;
it needs no example-specific Python script. Its `PYTHON` setting must select
the environment with CopperScript installed. In this checkout the CM4 local
Makefile also includes the shared recipes: `make -C examples/cm4_baseboard route`.

Successful routing is **not manufacturing signoff**. Power/current review,
mechanical/RF qualification, exact assembly selections and manufacturing export
remain separate obligations; this workflow does not create Gerbers or waive DRC.

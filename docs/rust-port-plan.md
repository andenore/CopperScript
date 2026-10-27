# Rust port plan

Status: draft 2026-10-27, for owner review (decisions in section 7 are open).
Owner: andenore. This plan does not change routing, placement or language
behaviour; algorithm work stays under the
[routing refactor plan](routing-refactor-plan.md) and the
[escape planning plan](escape-planning-plan.md).

## 1. Goal

One Rust implementation of the compiler and the physical pipeline, shipped as
a native `copper` binary, that produces the same outputs as today's Python
`pcbir` for the same inputs.

Why:

- **Run time.** Routing dominates: CopperAssetTracker takes 27–65 min, and a
  CopperProbe run has taken 13.4 h. Most of that is pure-Python search, object
  churn and geometry, not the 379-line C A* kernel.
- **Distribution.** Consumers need a uv-locked Python environment today. A
  binary (plus Python wheels for the API) removes that for CLI users.
- **Correctness.** Exact-integer geometry, ownership and immutability rules are
  easier to enforce with Rust types than by convention.

Non-goals: changing any algorithm during the port, changing the `.copper`
language, the report schemas, the state-file format or KiCad output bytes.

## 2. What there is to port

`pcbir` and `copperscript`: 215 files, about 84,400 lines of Python.

| Subsystem | Lines | Files | Recent churn |
|---|---:|---:|---|
| Routing: unified router, lattice, critical/pair routing, fanout, plane, bypass, flow | 30,100 | 58 | High (`unified_router.py` 80 commits since 2026-10-13) |
| Frontend: lexer, parser, compiler, model, ERC, packages, library | 8,600 | 15 | Low |
| Physical IR, exact geometry, mechanics, macros | 8,000 | 20 | Low |
| DRC, native verification, qualification, CAM | 7,500 | 22 | Low |
| Placement and congestion feedback | 6,800 | 10 | Medium |
| Editor, LSP, CLI, tooling | 4,900 | 24 | Medium (`cli.py`) |
| KiCad backends and importers | 4,400 | 10 | Low |
| Reports, issues, bench/ladder, state codec, misc | 13,900 | 55 | Medium |

Plus:

- **Tests:** 340 files, about 56,400 lines and 4,150 tests, all pytest.
- **Native code:** the C A* kernel `pcbir/native/astar.c`, shipped as prebuilt
  Linux and Windows libraries.
- **JavaScript:** the browser editor assets, the VS Code extension and its
  tests. The extension starts the compiler and the LSP as subprocesses.
- **Python dependencies:** numpy (6 modules: lattice, unified router,
  congestion map, plane estimate, power islands, state codec). Optional
  extras: shapely, pygerber and gerbonara (independent qualification and CAM
  checks), matplotlib and plotly (simulation plots), ortools (exact escape).

### The Python API is a product

Consumers import `pcbir` directly, not only the CLI:

| Consumer | Uses |
|---|---|
| CopperLib | `pcbir`, `placement`, `importers`, `physical`, `hard_macros`, `engineering_qualification` in tests and generators |
| CopperAssetTracker | `pcbir`, `hard_macros`, `placement`, `physical`, `loader`, `backends.kicad_pcb` in tests and scripts |
| CopperLedRing | `pcbir`, `hard_macros`, `placement`, `physical`, `backends.kicad_project` |
| CopperBench | `pcbir.workbench`, `backends.kicad_pcb`, `scene`, `manufacturing_files` |

The port therefore keeps a Python package with the same import paths for as
long as consumers use it (decision D2).

### What the Rust clearance prototype showed

Branch `prototype/rust-clearance-index` mirrored the clearance index hot path
in Rust behind the Python API: byte-identical boards, 1–13 % faster, and no
change on CopperAssetTracker. There, `_clear` was about 4 % of samples; time
went to the critical pair search's own geometry and object churn. Porting one
hot function at a time behind a fine-grained Python boundary does not pay. A
stage has to move to Rust as a whole, with its loop, its geometry and its data
structures, and cross the language boundary once per call.

## 3. Approach: strangler port behind PyO3, oracle-checked

Port stage by stage into a Rust workspace exposed to Python with PyO3. The
Python implementation of a stage stays as the reference until the Rust one
matches it. Each stage switches over only when it is equivalent:

1. **Same inputs, same outputs.** Fixture inputs go through both
   implementations. The outputs are compared exactly, as state-file JSON and
   as KiCad bytes.
2. **The existing suite keeps running.** The pytest suite runs against the
   Rust-backed stage through the Python API. The 4,150 existing tests become
   the acceptance suite with no rewrite.
3. **The ladder is unchanged.** All six benchmark-ladder boards
   (round-led-ring, nrf52, full-vertical, cm4, CopperLedRing,
   CopperAssetTracker) stay byte-identical, and run time is measured on each
   (AGENTS.md: measure every change with the benchmark ladder).

Once every stage is in Rust, the CLI becomes a Rust binary. The Python package
shrinks to bindings over the Rust core.

Why not a from-scratch rewrite: there is no oracle until everything is done,
the routing code changes weekly, and consumers would see one large break
instead of a sequence of identical releases.

### Boundaries

- **Coarse calls only.** For example `route_unified(board, guides, options) ->
  result`, `run_physical_drc(board) -> report` or `compile_source(text) ->
  design`. No per-query or per-node crossings.
- **Interchange.** A boundary passes data either as native Rust objects
  wrapped in Python classes, or through the existing state codec
  (`copperscript.state/v1` JSON) where a stage is still Python on one side.
  The codec format becomes the cross-language contract, so its schema is
  frozen and versioned from R0.
- **Callbacks.** Progress callbacks cross the boundary as events (phase, event
  and details), batched where they are frequent.

### Numerics and determinism

The outputs are byte-identical today; the port must keep them so:

- Lengths are integer nanometres in `i64`, with `i128` for products. Exact
  fractions use `num-rational`, and `Decimal` uses a fixed-point type with the
  same rounding (Python rounds half to even).
- Float code (for example `hypot`, `atan2`, cost weights) keeps the same
  operations in the same order. Results that feed output are checked bit for
  bit against Python, and math functions that differ between platforms are
  pinned.
- Iteration order is deterministic everywhere: `BTreeMap`/`IndexMap`, no
  `HashMap` iteration feeding output, and stable sorts with the same keys.
- Text output (KiCad files, reports, quantity formatting such as `100 nF`) is
  generated by the same rules and compared byte for byte.

## 4. Order: stable leaves first, moving routing last

Porting code that is still being rewritten doubles the work. Stable,
self-contained layers go first, which also builds the shared Rust types
everything else needs. The unified router and critical routing go after the
current routing refactor and escape planning work settle, or under a freeze
agreed for that phase (decision D3).

| Phase | Scope | Why here | Accept when |
|---|---|---|---|
| R0 Foundations | Cargo workspace (`crates/`), maturin and PyO3 build, Linux and Windows wheels in CI, a frozen state-codec schema, the numeric types, a differential-test harness (Python vs Rust on a fixture corpus), ladder timing hook | Everything else depends on it | A trivial ported function round-trips through the harness on both platforms |
| R1 Physical IR and exact geometry | `physical`, `geometry`, `surface_path`, `zone_geometry`, `breakout`, `routing_clearance` (absorb the prototype), outline and mechanics | Stable leaf; every later stage uses these types | Geometry and clearance tests pass on Rust; ladder identical |
| R2 DRC | `drc`, `process_drc`, `pad_stitch`, route closure checks | Self-contained, called in loops by later stages | DRC reports identical on the ladder and the test corpus |
| R3 KiCad I/O | `backends/kicad_pcb`, `kicad_project`, `importers/kicad_pcb`, `importers/kicad_mod`, footprint resolution | Stable; output bytes are the identity check | Generated `.kicad_pcb`/`.kicad_pro` byte-identical; the KiCad conformance fixtures pass |
| R4 Global routing and placement | `routing` (global router), `routing_lattice`, `layer_assignment`, `placement`, `routeflow`, `congestion_feedback` | Medium churn; large share of run time on the small boards | Ladder identical; placement checkpoints still load |
| R5 Unified router | `unified_router` plus the C A* kernel (retired) | Largest run-time share; needs R1, R2 and R4 | Ladder identical; time recorded per board |
| R6 Critical and pair routing | `critical`, `pair_search`, `pair_*`, tuning, `local_critical`, `escape_plan` | Dominates CopperAssetTracker; high churn | Ladder identical, the tracker included |
| R7 Pipeline stages and flow | `flow`, `fanout`, `decoupling*`, `plane`, `bypass*`, `filled_feedback`, `dependency_reroute`, `power_islands` | Glue across R1–R6 | Whole `run_routing_pipeline` in Rust; ladder identical |
| R8 Frontend | lexer, parser, compiler, model, ERC, packages and lockfiles (git fetching), library loading, quantities | Stable but large; the CLI needs it to drop Python | All compiler, ERC and package tests pass on Rust |
| R9 CLI, LSP, editor, reports | `copper` binary, LSP (for example `tower-lsp`), editor server, `run_issues`, `view` rendering, bench and ladder tools | Last user-facing pieces | VS Code extension and editor tests pass against the binary; consumer Make targets run unchanged |
| R10 Python surface | `pcbir` becomes bindings over Rust; Python implementations deleted | After every stage has switched | Consumer test suites pass on the bindings |

Each phase is one or more PRs. A phase ends with the Python implementation of
that stage deleted, not kept as a second path.

### Staying in Python

The independent qualification and CAM checks (`cam_*`, `rf_*`,
`engineering_qualification`, `stackup_qualification`) use shapely, pygerber
and gerbonara on purpose: they are separate implementations that check
CopperScript's output. They stay in Python and call the Rust core through the
bindings (decision D4). The ngspice and plotting glue also stays Python.

## 5. Effort

Rust is usually about the same length as this kind of Python, or up to 1.5
times longer. Very roughly, at one experienced Rust engineer full time, with
review:

| Phase | Estimate |
|---|---|
| R0 | 2–3 weeks |
| R1–R3 | 6–9 weeks |
| R4–R5 | 6–9 weeks |
| R6–R7 | 8–12 weeks |
| R8–R10 | 6–9 weeks |

That is about 7–10 months in total. The routing phases carry most of the
uncertainty, both in size and in how much the code changes underneath them.
AI-assisted porting with the differential harness can shorten the mechanical
part. Byte-identity debugging and review do not shrink as much.

## 6. Risks

| Risk | Mitigation |
|---|---|
| Floating-point or rounding differences break byte identity | Same operation order; bit-level comparison in the harness; pin platform math; exact integer types wherever Python uses `int` or `Fraction` |
| Two implementations drift while a stage is half ported | One stage in flight at a time; a stage switches and its Python implementation is deleted in the same phase |
| Routing changes land while R5–R7 are in progress | Port those after the refactor settles, or freeze those modules for the phase (D3) |
| Consumers break on API changes | Same `pcbir` import paths through PyO3 classes; run the consumer suites in CI before each release |
| Windows builds (the current authoring environment is Windows) | Build and test Windows wheels and binaries from R0 on |
| The boundary costs more than the work it saves | Coarse calls only; measure each switch on the ladder before accepting it |
| Pytest relies on Python internals (monkeypatching private functions) | Treat such tests as implementation tests: move them to Rust unit tests when their module is ported |

## 7. Decisions for the owner

- **D1 Approach.** Strangler port behind PyO3, checked against the Python
  implementation as above (recommended), or a clean rewrite.
- **D2 Python surface.** Keep `pcbir` as a permanent Python binding layer
  (recommended, because consumers use it), or retire it after migrating the
  consumers to the CLI or a new API.
- **D3 Routing timing.** Start R5–R7 only after the routing refactor and
  escape planning steps settle (recommended), or freeze those modules while
  they are ported.
- **D4 Independent checks.** Keep the shapely, pygerber and gerbonara
  qualification checks in Python (recommended), or port them too.
- **D5 Identity bar.** Byte-identical ladder boards required for every phase
  (recommended), or allow measured, reviewed differences where Python
  behaviour is itself a bug.

## 8. First steps

1. Owner decisions D1–D5.
2. R0: the workspace, the build, the CI wheels and the harness, with one
   trivial function ported end to end on Linux and Windows.
3. R1: absorb `prototype/rust-clearance-index` into the shared geometry crate,
   and port the physical IR types it needs.

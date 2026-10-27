# Rust rewrite plan

Status: revised 2026-10-27 after owner decisions (section 8); M0 accepted
(section 10). Owner: andenore.
This plan does not change routing, placement or language behaviour on the
Python side; algorithm work there still follows the
[routing refactor plan](routing-refactor-plan.md) and the
[escape planning plan](escape-planning-plan.md) until section 5 says otherwise.

## 1. Goal

One Rust implementation of the compiler and the physical pipeline, shipped as
a native `copper` binary, that produces equivalent outputs to today's Python
`pcbir` for the same inputs. The CLI is the product. There is no Python API.

Why:

- **Run time.** Routing dominates: CopperAssetTracker takes 27–65 min, and a
  CopperProbe run has taken 13.4 h. Most of that is pure-Python search, object
  churn and geometry, not the 379-line C A* kernel.
- **Distribution.** Consumers need a uv-locked Python environment today. A
  binary removes that.
- **Correctness.** Exact-integer geometry, ownership and immutability rules are
  easier to enforce with Rust types than by convention.

Non-goals: changing the `.copper` language, the report schemas, the state-file
format or the KiCad output structure. Behaviour may differ where the Python
implementation is itself wrong, and only when the difference is reviewed and
recorded (section 4).

## 2. What there is to rewrite

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

What happens to the rest:

- **Tests.** The 340 pytest files (56,400 lines, 4,150 tests) are not ported.
  They are development scaffolding for the Python implementation. Their
  fixture inputs (`.copper` sources, footprints, KiCad files, state files)
  become the golden corpus (section 4); their assertions are replaced by Rust
  unit tests written with each crate and by CLI-level comparison on the
  corpus.
- **Native code.** The C A* kernel `pcbir/native/astar.c` is retired. The
  router is Rust end to end.
- **JavaScript.** The browser editor assets and the VS Code extension stay.
  The extension starts the compiler and the LSP as subprocesses, so it
  switches from `python -m copperscript` to the binary.
- **Python dependencies.** numpy is replaced by Rust code. The optional extras
  (shapely, pygerber, gerbonara, matplotlib, plotly, ngspice glue) stay with
  the independent checks, which are out of the first iteration (section 5).

## 3. Consumers: the CLI is the only surface

The consumer repositories were inventoried at their current heads
(CopperLedRing 0c4d11d, CopperAssetTracker 8ac4fea, CopperLib 1e1b8a8,
CopperBench e8bccab; CopperProbe is empty).

| Consumer | Files importing `pcbir` | What they are |
|---|---:|---|
| CopperLedRing | 0 | Makefile only: `check`, `export-kicad-pcb`, `edit-mechanical`, `route-board`, `assembly`, `export-manufacturing` |
| CopperAssetTracker | 13 | 5 test files and 8 scripts under `scripts/`; the Makefile uses `lock`, `check`, `audit-footprints`, `compile`, `plan-layout`, `edit-mechanical`, `route-board`, `assembly`, `export-manufacturing`, `audit-cam`, `assess-engineering`, `qualification-gate` |
| CopperLib | 44 | 43 pytest files and `layout_trial.py` for the TPS62130A buck macro |
| CopperBench | 7 | Read-only tools under `full-board/tools`, `prototype` and `tools` |

**Rule (decision D2).** A `pcbir` import in a consumer is removed, not given a
CLI equivalent. The rewrite does not grow its command surface to carry
consumer test or development code:

- A check of generic CopperScript behaviour (hard-macro materialisation,
  pad-shape clearance, scene geometry, plane stitching) moves into
  CopperScript's own tests, written once in Rust, and is deleted from the
  consumer.
- A check of the consumer's own content (a package's pin contract, a board's
  assembly lock, an ERC result) that an existing command already exposes
  (`check`, `compile` JSON, `audit-footprints`, `assembly`) is rewritten to
  drive the CLI and read its JSON.
- Everything else was intermediate testing or development tooling and is
  deleted.

Disposition by kind of use, most common first:

| Use in consumers | Where | Disposition |
|---|---|---|
| Compile a tiny board and inspect the model (`compile_source`, `check`, `elaborate`, `board_to_dict`) | About 30 CopperLib test files, 2 tracker tests | Drive `copper compile`/`copper check`; keep only assertions about the package's own facts |
| Parse a KiCad footprint (`load_kicad_mod`) to check pad geometry | About 12 CopperLib tests | Delete; footprint parsing is CopperScript's to test |
| Physical trials around a hard macro (physicalize, materialise/validate macros, DRC, pad transforms, KiCad project export) | 11 TPS62130A macro tests, `layout_trial.py`, tracker `probe_power_macros.py`, `build_battery_revision.py`, power-layout tests | Delete from consumers; generic macro validation moves into CopperScript's Rust tests, using the `tps62130a_macro` and `nrf_antenna_macro` examples |
| Geometry queries on routed output (`workbench`, `scene`, `kicad_reference`, power-path width) | All 7 CopperBench tools, 1 tracker script | Delete; `copper issues` and `copper view` are the supported inspection path |
| Qualification contracts, assembly locks, manufacturing files | Tracker scripts and tests, 3 CopperLib tests | Drive `assembly`, `export-manufacturing` and the qualification commands; delete the rest |
| Private names (`pin_escape._verified_land_path`, `importers.kicad_mod._parse_sexpr`) | 2 tracker scripts | Delete; board-specific routing checks are already disallowed in consumers |

These removals are PRs in the consumer repositories. They are a prerequisite
for deleting the Python core (M7) and can start now, since each one only
removes code or switches a test to commands that already exist.

## 4. Approach: clean rewrite, checked against the Python CLI

Write the Rust implementation from the specification and the Python source,
in a Cargo workspace under `crates/`, producing one binary. The Python
implementation is not modified for the rewrite and is not wired to it; it
stays in the repository as the reference until the binary passes acceptance,
then is deleted (except section 5's independent checks).

Why a clean rewrite rather than a stage-by-stage port: the owner wants one
coherent Rust design rather than Python structure mirrored through a foreign
function boundary, and consumers will not import the result, so there is no
intermediate API to keep working. The cost is that nothing ships until a
whole command is equivalent; milestones are therefore cut per CLI command
(section 5) so each one has an oracle.

### Oracle and golden corpus

- The oracle is the Python CLI at a pinned commit, run through
  `python -m copperscript`.
- The corpus is every input the repository already has: the `.copper`
  sources, footprints, KiCad files and state files under `tests/` and
  `examples/`, the six benchmark-ladder boards (round-led-ring, nrf52,
  full-vertical, cm4, CopperLedRing, CopperAssetTracker at pinned commits)
  and the consumer boards.
- The harness, `scripts/compare_implementations.py`, runs the same command
  on both implementations and compares exit status, stdout, stderr and every
  output file byte for byte: compiled design JSON, ERC diagnostics,
  `copperscript.state/v1` files, DRC and route reports, generated
  `.kicad_pcb`/`.kicad_pro` bytes, manufacturing files. Both runs use the
  same working and output directories, so paths in the output agree. The
  oracle runs with `PYTHONUTF8=1` and `PYTHONHASHSEED=0`.
- The corpus manifest, `scripts/compare_corpus.json`, lists each suite of
  commands with the milestone that rewrites it. Suites up to its
  `accepted_milestone` are gated in CI; later suites report `pending`.
- Unit tests check the foundation crates against fixtures recorded from the
  oracle by `scripts/generate_rust_fixtures.py` (decimal arithmetic, float
  `repr`, JSON escaping, quantity text, real state files). CI fails when a
  fixture is stale, so a Python change that alters these shows up at once.
- Ladder run time is recorded for both implementations on every milestone
  (AGENTS.md: measure every change with the benchmark ladder).

### Differences (decision D5)

Byte identity is the default expectation and the harness reports every
deviation. A deviation is accepted only when it is reviewed and recorded in
`docs/rust-port-differences.md` with the input, the two outputs, the reason
the Python behaviour is wrong or arbitrary, and the milestone that accepted
it. Unrecorded differences block the milestone. The ladder boards must still
route completely and pass native DRC; their metrics (length, via count,
completion time) may differ within the recorded entries.

### Numerics and determinism

- Lengths are integer nanometres in `i64`, with `i128` for products. Exact
  fractions use `num-rational`; fixed-point quantities keep their
  `base_value` and the shared formatter rules (`100 nF`). `Decimal`
  reproduces Python's default context: 28 significant digits, round half
  even, Python's exponent rules for every operation and for `str()`.
- Line endings: the binary writes LF on every platform (decision of
  2026-10-27). Python's text mode (`print`, `write_text`) writes `\r\n` on
  Windows; the harness reads the Python outputs' CRLF as LF on Windows before
  comparing, so this is a policy, not a per-case recorded difference.
- Float code (`hypot`, `atan2`, cost weights) keeps the same operations in
  the same order unless a recorded difference says otherwise.
- Iteration order is deterministic everywhere: `BTreeMap`/`IndexMap`, no
  `HashMap` iteration feeding output, stable sorts with the same keys. The
  binary must reproduce its own output bit for bit across runs and platforms.

### Boundaries inside the Rust workspace

One crate per layer. The foundation, from M0: `copper-json` (Python-exact
JSON), `copper-units` (decimal, nanometres, fractions, quantities) and
`copper-state` (the state format). Then `copper-lang` (frontend),
`copper-physical` (IR, geometry, footprints, macros, mechanics),
`copper-kicad` (I/O), `copper-drc`, `copper-place`, `copper-route`,
`copper-report` (issues, view, summaries), `copper-lsp`, and the `copper`
binary. Crate boundaries follow the current
module groups so the Python source can be read alongside.

## 5. Milestones, in dependency order

Each milestone is one or more PRs and ends with a ladder run and the
differences log updated. A milestone is accepted when the harness shows the
named commands equivalent on the corpus, with every deviation recorded.

| Milestone | Scope | Commands equivalent | Accept when |
|---|---|---|---|
| M0 Foundations | Cargo workspace, Linux and Windows binaries in CI, numeric types, state-codec schema frozen, the comparison harness and differences log, ladder timing hook | `agent-guide`, `state` | The harness compares the M0 commands on both platforms |
| M1 Frontend | Lexer, parser, compiler, model, ERC, quantities, packages and lockfiles (git fetching), library loading, power-state analysis | `check`, `compile`, `lock`, `audit-footprints`, `library`, `compile-macro`, `query`, `power-check` | Every `.copper` source in the corpus compiles to equivalent JSON; ERC diagnostics equivalent |
| M2 Physical IR and KiCad I/O | `physical`, `geometry`, `surface_path`, `zone_geometry`, `breakout`, footprints and `kicad_mod` import, outline and mechanics, hard-macro materialisation, `kicad_pcb`/`kicad_project` backends and the board importer | `export-kicad-pcb`, `export-kicad`, `check-footprint` | KiCad output equivalent for the examples and consumer boards; macro examples materialise |
| M3 DRC, placement, assembly | `drc`, `process_drc`, `pad_stitch`, `placement`, `clusters`, `congestion_feedback`, `layout`, assembly locks and manufacturing files (without the independent CAM audit), editor backend | `plan-layout`, `edit-mechanical` (and its alias `edit`), `si-check`, `assembly`, `export-manufacturing --skip-independent-cam` | Placement and DRC reports equivalent on the ladder; editor tests pass against the binary |
| M4 Routing | Global router, `routing_lattice`, `layer_assignment`, `routeflow`, `unified_router` (A* kernel retired), `critical`, `pair_*`, `escape_plan`, `fanout`, `decoupling*`, `plane`, `bypass*`, `filled_feedback`, `dependency_reroute`, `power_islands`, `flow` | `route-board`, `route-global`, `replay-unified`, `replay-critical`, `benchmark-routing`, `bench` (the ladder runner) | All six ladder boards route completely and pass native DRC; metrics within the recorded differences; run time recorded per board |
| M5 Diagnostics and reports | `run_issues`, `view` rendering, `explain`, `summarize`, `routing_benchmark` summaries, the third-party board benchmark tool (`python -m pcbir.bench`) | `issues`, `view`, `explain`, `summarize` | Reports equivalent on the ladder runs |
| M6 LSP and editor | `lsp` (for example `tower-lsp`), editor server, VS Code extension pointed at the binary | `lsp`, `editor-overlay` | Extension and editor tests pass against the binary |
| M7 Cutover | Consumer `pcbir` imports removed (section 3), consumer Makefiles on the binary, Python core deleted, the pinned oracle commit recorded | all of the above | Consumer Make targets run unchanged except for the binary path; `pcbir` reduced to section 5's remainder |

### Out of the first iteration (decision D4)

The independent qualification and CAM checks (`cam_*`, `rf_*`,
`engineering_qualification`, `stackup_qualification`, `qualification_gate`,
the native qualification and RF probes) and the ngspice simulation glue are
not rewritten in this iteration. They stay as the Python commands `audit-cam`,
`assess-engineering`, `qualification-gate` and `sim`, reading the binary's
outputs (KiCad files, state files, reports) from disk. The binary answers
these commands with a pointer to `python -m copperscript`.

They import core `pcbir` modules (`physical`, `placement`, `pair_geometry`,
`power_integrity`, `route_path_geometry`, `rf_geometry`, `engineering`), so
at M7 the Python package is reduced to those modules plus the check code,
installed only with the `qualification` extra, and frozen: no further
development on the Python side. Rewriting them is a later iteration.

### Routing work during the rewrite (decision D3)

The rewrite includes routing (M4) without waiting for the routing refactor or
escape planning to finish. Consequence: every algorithm change that lands in
Python after M4 starts has to be mirrored in Rust before M7, or dropped. The
recommendation is that algorithm work moves to the Rust router once M4 is
accepted and the Python router is frozen from that point; until then, the
Python-side plans stay in force and M4 is written against the Python router
as it stands when M4 starts.

## 6. Effort

Rust is usually about the same length as this kind of Python, or up to 1.5
times longer. Very roughly, at one experienced Rust engineer full time, with
review:

| Milestone | Estimate |
|---|---|
| M0 | 2 weeks |
| M1 | 4–6 weeks |
| M2–M3 | 6–9 weeks |
| M4 | 12–18 weeks |
| M5–M6 | 4–6 weeks |
| M7 and consumer PRs | 2–3 weeks |

That is about 7–10 months in total, with no shippable binary before M1 and no
replacement for a consumer build before M4. The routing milestone carries
most of the uncertainty, both in size and in how much the Python router
changes underneath it. AI-assisted drafting against the harness can shorten
the mechanical part; reviewing differences and debugging routing divergence
do not shrink as much.

## 7. Risks

| Risk | Mitigation |
|---|---|
| No oracle for partial work | Milestones are cut per CLI command so every milestone has one; the harness runs from M0 |
| Floating-point or rounding differences cascade in routing | Same operation order by default; the differences log forces each deviation to be understood; integer types wherever Python uses `int` or `Fraction` |
| Routing changes land in Python while M4 is in progress | Freeze the Python router when M4 is accepted (section 5); until then, mirror or drop each change explicitly |
| Consumers lose checks they relied on | Section 3 moves generic checks into CopperScript's tests and keeps content checks on existing commands; nothing consumer-specific is carried |
| Windows builds (the current authoring environment is Windows) | Build and test Windows binaries in CI from M0 |
| The reduced Python package for independent checks rots | Freeze it at M7 with a pinned lock; schedule its rewrite as the next iteration |
| Reviewed differences hide regressions | Every entry names input, outputs and reason; the ladder must still route completely and pass native DRC |

## 8. Decisions (recorded 2026-10-27)

- **D1 Approach: clean rewrite.** No PyO3 strangler port; the Python
  implementation is the reference only.
- **D2 Python surface: none.** Consumers do not import `pcbir`. Their current
  imports are removed as intermediate testing or development code, not given
  CLI equivalents (section 3).
- **D3 Routing timing: include routing in the rewrite now.** Do not wait for
  the routing refactor or escape planning to settle (section 5).
- **D4 Independent checks: not in the first iteration.** They stay as Python
  commands on a reduced, frozen package (section 5).
- **D5 Identity bar: reviewed differences allowed.** Byte identity is the
  default; every deviation is recorded in the differences log (section 4).

## 9. Next steps

1. Consumer PRs under section 3, starting with CopperLib (largest count) and
   CopperBench (all deletable); confirm each repository's dispositions with
   its owner before deleting.
2. M1: the frontend, measured by `check` and `compile` on the whole corpus
   (in progress, section 10).

### Open items

1. **CLI usage, help and error text (decided at the start of M1).** Python's
   argparse wording (usage wrapping, `invalid choice` lists, `-h` output) is
   not reproduced. The binary keeps argparse's exit statuses, and those texts
   stay out of the corpus; a consumer that depends on one gets a recorded
   difference.
2. **Entry points without a milestone.** `copper-devicegen` (device model
   generation) has no milestone yet; `copper-lsp` becomes `copper lsp` in M6.
   Decide before M7.

## 10. Status

**M0 (2026-10-27): accepted.** Rust workflow run 40472862859 passed on
`ubuntu-24.04` and `windows-latest` at commit e8ce9b3.

- Workspace: `Cargo.toml`, `rust-toolchain.toml` (1.97.0), `crates/` with
  `copper-json`, `copper-units`, `copper-state` and the `copper` binary; see
  [crates/README.md](../crates/README.md).
- The binary implements `agent-guide` and `state`; every other Python
  command reports its milestone and exits 3.
- Oracle fixtures: 2,716 decimal operations, 630 floats, quantity text for
  every unit, JSON escaping and documents, and three state files (two
  physicalized example boards and one value using every tag) round-trip
  byte for byte.
- Harness and corpus: 111 cases in 8 suites (M0 18, M1 64, M2 11, M3 11,
  ladder 7). All 18 gated M0 cases are byte-identical on Linux and Windows,
  including Windows path text. (CRLF output was matched for M0; LF on every
  platform was decided later, see section 4.)
- [Differences log](rust-port-differences.md): empty.
- CI: `.github/workflows/rust.yml` runs format, clippy, tests, the fixture
  check and the gated comparison on `ubuntu-24.04` and `windows-latest`, and
  uploads the binaries and the comparison report.
- Ladder: the `ladder` suite is the timing hook. M0 does not change the
  Python implementation, so the ladder was not rerun for it.

**M1 (2026-10-27): complete locally; gated in CI from this change.**

- Commands rewritten: `check`, `compile`, `query`, `power-check`,
  `audit-footprints`, `lock`, `compile-macro` and `library`.
- `copper-lang` parses every CopperScript document kind with Python's
  diagnostics (1,118 recorded parse outcomes match; all 146 CopperLib and 6
  CopperAssetTracker sources match locally) and rewrites package resolution
  and lockfiles, electrical, mechanical and silkscreen lowering, ERC, the
  compiled JSON, design queries, power-state analysis, footprint resolution
  and audit, macro lowering and asset preparation, and library discovery.
  `copper-physical` holds the exact board geometry, the footprint model and
  the `.kicad_mod` importer.
- Corpus: 1,723 gated cases (M0 and M1). 1,716 are byte-identical and 7
  are recorded differences (D-0001 to D-0007: Python crashes on a component
  with an unknown part). Locally, 199 targeted mechanical and silkscreen
  snippets and about 1,200 further seeded mutations of the mechanical
  boards, each run through `check --json` and `compile`, also match.
- Exactness: Python's 60- and 80-digit decimal steps (curve chord limits,
  edge-attachment offsets, DXF lengths, reference transforms) are computed
  with exact rationals, which agree wherever those digits can tell the
  results apart. Lengths beyond 2^40 nm (1.1 km) report "not implemented".
- Decisions: exit statuses stay Python's, argparse wording is not copied
  (usage-error cases compare exit status and stdout only); the binary
  writes LF everywhere (section 4); an uncaught Python exception is
  compared by its final line, so its reviewed difference matches on every
  platform.
- CI populates each board's package cache with a locked online
  `copper check` and compares with `--jobs 4`. The KiCad footprint
  repository stays uncached, so footprint resolution through it reports the
  offline cache miss in both implementations.
- Next: M2 (`export-kicad-pcb`, `export-kicad`, `check-footprint`).

**M2 (in progress).**

- `export-kicad`: the KiCad schematic backend (`copper-lang`
  `kicad_schematic`: flat single-sheet schematic, embedded symbols, the
  project-local symbol library, `sym-lib-table` and `.kicad_pro`) and its
  writer. The `export-kicad`, `export-kicad-no-check` and
  `export-kicad-outputs` suites (374 cases: every example and fixture board,
  ERC failures with and without `--no-check`, output-path forms) are
  byte-identical.

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
  the same order where that is natural, but floating-point rounding is not
  reproduced bit for bit (decision D6): no Python or numpy numeric algorithm
  is ported only to match its last bit. The harness reports outputs that
  differ only in float rounding as `close` (a pass): exit statuses equal,
  text equal outside its numbers, integers equal, floats within a relative
  1e-9, JSON compared as values; `--exact` shows the byte comparison. The
  exact helpers already written (`pyfloat`) stay in use.
- Where a rounding difference changes a decision (a routing tie, a legality
  threshold), outputs diverge beyond `close`. For routing commands the
  acceptance is then quality, not identity: the Rust result must route as
  completely, pass DRC, and be no worse on the benchmark ladder's metrics
  (completion, length, vias); such cases are recorded with those metrics.
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
- **D6 Floats: rounding is not reproduced (recorded 2026-10-28).** Backwards
  compatibility will break anyway, so bit-exact float parity is not worth
  porting numeric algorithms for. Differences only in float rounding pass as
  `close`; routing that diverges because of them is judged on quality
  (section 4, numerics).

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

**M2 (2026-10-27): complete; gated in CI on Linux and Windows.**

- Commands rewritten: `check-footprint`, `export-kicad`, `export-kicad-pcb`.
- `copper-lang` gains the KiCad schematic backend (flat single-sheet
  schematic, embedded symbols, the project-local symbol library,
  `sym-lib-table` and `.kicad_pro`). `copper-pcb` holds the physical board IR,
  physicalisation with proxy and resolved footprints, placement templates,
  hard-macro scenes, width contracts and edge openings, and the KiCad
  PCB/project backend with tracks, vias, fixed polygons and the via-process
  sidecar. `copper-drc` (pulled forward from M3, because M2 requires macro
  examples to materialise) holds the physical DRC on that path, exact copper
  connectivity, breakout-region spacing, placement legality and hard-macro
  materialisation with its port, plane-return and private-net proofs.
- Corpus: 12 M2 suites, 1,484 cases (every example, fixture, footprint and
  synthetic macro board, every option, error paths and seeded mutations):
  1,458 byte-identical and 26 recorded differences (D-0008 to D-0033, all
  Python crashes on a component with an unknown part). The full gate (M0 to
  M2) is 3,174 equal and 33 recorded.
- Consumer boards: CopperLedRing's `export-kicad-pcb` is byte-identical in
  every file; CopperAssetTracker's matches (its macros stop before
  placement in both). With its macros pinned, the three real CopperLib
  macros materialise and reproduce Python's DRC report byte for byte.
- Windows: Python's two OS-error forms (C runtime `[Errno N]` for file I/O,
  `[WinError N]` for `os` calls) and `str(Path(arg))` for option paths are
  reproduced (`copper_json::oserror`).
- The oracle needs shapely (macro edge openings), installed with it in CI.
  Python oracle results are cached by the non-Rust tree, so Rust-only
  changes rerun only the binary.
- Not implemented (exit 3, no corpus case reaches them): via-in-pad via
  arrays, non-numeric macro rotations, non-string scene fields. The DRC
  signoff token, waivers, relaxation records and zone-fill checks are M3.
- Next: M3, M5 and M6 run in parallel streams (`rust-m3-layout`,
  `rust-m3-assembly`, `rust-m5-reports`, `rust-m6-lsp`); M4 follows
  placement.

**M6 `copper lsp` (stream `rust-m6-lsp`): complete locally.**

- `crates/copper-lsp` rewrites `pcbir.lsp` on the `copper-lang` frontend:
  Content-Length framing with Python's transport errors, JSON-RPC dispatch
  with Python's error codes and `str(exc)` messages (malformed parameters
  fail with the same `KeyError`/`TypeError` text), the 150 ms debounced
  diagnostics worker publishing only current snapshots, the recovering
  parse on token slices (`copper_lang::parse_tokens`), symbols, hover,
  definition, completion, compiler-backed diagnostics with fixes, and code
  actions. The compiler and macro preparation read open buffers through a
  `source_reader` on the package resolver (`compile_design_source_with`,
  `prepare_macro_bindings_with`).
- Harness: a case may pipe a client transcript (`stdin`, JSON lines of
  `send`, `raw` and `wait` steps, framed by the harness; `{root_uri}` and
  suite-input placeholders). `wait N` blocks until N complete server
  messages arrived, so debounced notifications land at fixed points; an
  unmet wait is a timed-out run. The transcript digest is part of the
  oracle cache key. LSP stdout is binary on both sides (CRLF headers by
  protocol, LF-free JSON bodies), so the Windows CRLF reading is not applied
  to it; stderr still is.
- Corpus: `lsp-sessions` (26 transcripts from
  `tests/rust-port/lsp/make_transcripts.py`: lifecycle, unsaved edits,
  Unicode/CRLF and rejected updates, quick fixes, packages, physical macros,
  JSON macro assets, 16 malformed transports, end of input), `lsp-sources`
  (every board with 3 seeded mutations through `board.jsonl`, a full
  editing session) and `lsp-library-sources` (package sources, never
  mutated because other boards inventory them): 359 cases, all
  byte-identical locally. The M2 gate still passes.
- URIs follow `urlsplit`/`url2pathname` (posix, and `nturl2path` on
  Windows with VS Code's `%3A` drive) and `Path.as_uri()`; replies use the
  client's spelling of an opened document. `Path` keys compare
  case-insensitively on Windows.
- `tests/lsp-client.cjs` runs the server from `COPPER_TEST_LSP_BINARY`
  when set (the extension itself is unchanged; its interpreter probe still
  uses `COPPER_TEST_PYTHON`): all five tests pass against the binary.
- Requests reuse one analysis per document and generation, as Python's
  `Workspace.cache` does. Not reproduced, because no client message can
  observe it: Python's dependency snapshots (`SnapshotResolver`, reused
  imports across generations); the binary re-resolves imports for each new
  analysis with one shared resolver. Only a package file edited on disk
  without a save or watched-file notification could tell them apart.
  Python analysis crashes (`LSP_ANALYSIS`) where the Rust frontend reports
  a located error are the recorded-crash class; none occurs in the corpus.
- `editor-overlay` (M6) needs routed build runs (M4) and remains.

**M5 diagnostics and reports (stream `rust-m5-reports`, in progress).**

- Commands rewritten: `explain`, `issues`, `view`, `summarize`, in the new
  `copper-report` crate (explanations, the scene reader for `.kicad_pcb`
  boards and physical-board state files, issue grouping, the view legend and
  SVG, the engineering summary). Loosely typed report JSON is read with
  Python's semantics, including the messages of the exceptions the CLI
  reports; `copper-json` gained a strict parse for `summarize`'s
  duplicate-key and non-finite hooks.
- `view` writes the SVG that the Python command writes without Pillow, the
  oracle's configuration in CI (`pip install -e ".[test]" shapely` installs
  no imaging library), including its `(Pillow not installed: wrote SVG)`
  legend line. A Python oracle with Pillow writes a PNG whose text is
  rasterized from platform fonts; that is not reproduced. Run the M5 suites
  locally with a Pillow-free interpreter (`--python` pointing at a fresh
  venv).
- Fixtures: `tests/rust-port/reports` (runs produced by the Python CLI on
  small example boards, derived reports for branches those boards do not
  reach, a handwritten scene board, malformed reports); see its README for
  the generating commands.
- Corpus: 748 cases in 10 M5 suites (`explain`, `issues`, `issues-json`,
  `issues-bad`, `view`, `view-boards`, `view-states`, `summarize`,
  `summarize-bad`, `summarize-json`): 746 byte-identical and 2 recorded
  differences (D-0034, D-0035: Python crashes on a malformed view target).
  `copper-json` now names Python 3.13's trailing-comma errors.
- Inputs the Rust reader does not model (a KiCad layer or net name written as
  a list, a malformed state file, a decimal tuple as a track length) exit 3
  as not implemented.

- `export-kicad`: the KiCad schematic backend (`copper-lang`
  `kicad_schematic`: flat single-sheet schematic, embedded symbols, the
  project-local symbol library, `sym-lib-table` and `.kicad_pro`) and its
  writer. The `export-kicad`, `export-kicad-no-check` and
  `export-kicad-outputs` suites (374 cases: every example and fixture board,
  ERC failures with and without `--no-check`, output-path forms) are
  byte-identical.
- `check-footprint`: the `.kicad_mod` import report. The `check-footprint`
  and `check-footprint-strict` suites (269 cases: every footprint in the
  examples, fixtures and cached CopperLib packages, with seeded mutations)
  are byte-identical.

**M3 assembly, si-check, export-manufacturing (stream `rust-m3-assembly`).**

- `assembly snapshot|check|bom|candidate-bom` (`copper-assembly`
  `assembly`): lock loading with Python's validation order and messages
  (including `TypeError` text for missing fields and repeated JSON keys),
  the electrical digest, the check report, the JLCPCB BOM, hand-assembly
  and ordering lists and the candidate review bundle with its CPLs. CSV
  files follow Python's `csv` (CRLF rows everywhere; the harness now reads
  both implementations' CRLF as LF on Windows). Lock, position and board
  fixtures live under `tests/rust-port/assembly` (`generate.py` rebuilds
  them with the oracle).
- `si-check` (`copper-drc` `signal_integrity`): closed-form impedance with
  Python's float operations and `round(x, 9)`, reference planes, layer
  groups and length-match screening, text and JSON. SI fixture boards are
  under `tests/rust-port/si-check`.
- `export-manufacturing --skip-independent-cam` (`copper-assembly`
  `manufacturing`): the whole pipeline, including a zlib-exact raw DEFLATE
  and `zipfile`-exact archives (checked against CPython's zlib on about 450
  inputs). KiCad 10 could not be installed in the authoring container (its
  PPA is blocked by the network policy; running the downloaded official
  `kicad/kicad:10.0` image binary was not permitted), so the suite drives
  `tests/rust-port/manufacturing/kicad-cli/fake-kicad-cli.sh`, a shell
  stand-in that writes plausible outputs and injects one failure per
  wrapper, through `{root}` (new harness placeholder). On Windows the stub
  cannot execute and both implementations report `[WinError 193]`.
- Not verified against real KiCad: CI would need KiCad 10 (`ppa:kicad/kicad-10.0-releases`
  on ubuntu-24.04, or the `kicad/kicad:10.0` container). Real `kicad-cli`
  writes dated Gerber attributes and DRC reports, so such a suite can only
  compare exit status, stdout and the deterministic files (`manifest.json`,
  `bom.csv`, `cpl.csv`). Staging directories carry random `mkdtemp` names;
  an OS error naming a file inside one (a `kicad-cli` that exits 0 without
  writing `drc.json`) cannot match and is left out of the corpus.
- Corpus (15 suites, 3,293 cases): 3,248 byte-identical and 45 recorded
  (D-0034 to D-0078: Python crashes in `si-check` on a component with an
  unknown part). The M2 gate still passes.

**M3 stream: `plan-layout` (branch `rust-m3-layout`).**

- `copper-place` ports the planner (seeds, analytical placement,
  legalization, rigid clusters, refinement, relative-constraint repair,
  escape spacing, candidate selection), placement-stage requirement
  verification, the physical board digest and the engineering summary.
  `copper-drc` legality gained the planner's policy (`Checker`), and the
  physical IR gained placement groups, power domains and feeds,
  decoupling links and attachments. Python's `math.hypot` and float `sum`
  are reproduced bit for bit (`copper-physical` `pyfloat`).
- Corpus: the `plan-layout` acceptance suite (11 boards, `--candidates 1`)
  is byte-identical, report and `placement.summary.json` included. Eight
  further suites (proxy inputs, fixtures, macros, six-layer macros, edge
  openings, options, error paths, templates) bring the total to 503
  cases: 480 byte-identical, 15 recorded Python crashes (D-0034 to
  D-0048) and 8 pending.
- Not implemented (exit 3): the decoupling-access phase when a movable
  decoupler needs the decoupler router (M4), non-zero
  `--support-pattern-trials`/`--package-support-trials`, via-in-pad
  arrays, routing-stage requirements and route-report summaries.
- Windows: Python digests the report it wrote with CRLF, so the summary's
  `report_sha256` differs from the Rust binary's LF report there; it needs
  recording once a Windows run has the digests.

**M4 stream: routing foundation and `route-global` (branch `rust-m4-global`).**

- `copper-route` is the base crate of the M4 routing streams: one module
  per Python module (`routing_clearance`, `routing_lattice`, `routing`,
  `layer_assignment`, `routeflow`, `escape_plan` and their helpers). `route-global`
  reuses plan-layout's physical pipeline and option parsing, then runs
  the transactional placement/global-routing feedback loop and writes
  the `copperscript-global-route/v0.1` guides and the optional placed PCB.
- `routing_lattice` (uniform and nested lattices) and `escape_plan`
  (diagnostics, pad fields, gaps and tiles, the two-end plan, bus layers,
  array min-cost flow) are checked against Python-generated fixtures
  (`crates/copper-route/tests/fixtures/{lattice,escape}.json`; escape
  boards resolve from the repository, so CI needs no package cache): every
  recorded value is identical, floats bit for bit.
- Corpus: six `route-global*` suites (every corpus board with its
  `{physical}` args, proxy inputs, fixtures, macros, options, error paths)
  with 456 cases: 435 byte-identical (guides, PCB project and stdout), 13
  recorded Python crashes on unknown parts (D-0096 to D-0108) and 8
  pending. No case depends on timing; route-global prints no progress.
- Not implemented (exit 3): what plan-layout lacks (the decoupling-access
  phase with a movable decoupler), `admit_fixed_prefix` (fixed copper
  prefixes, used by route-board).
- cm4 and nrf52 need the KiCad footprint repository in their package
  cache; where it is missing both implementations report the same
  offline cache miss.

**M4 stream: pin access, planes, the routing pipeline and `route-board`
(branch `rust-m4-flow`).**

- Built on `rust-m4-global` (merged: clearance index, global router, layer
  assignment). `copper-route` adds `progress` (`ConsoleProgress` on the
  global stream's `emit`), `ladder` (`copper bench`), `routing_acceptance`
  (`copper benchmark-routing`), `drc_report`, `plane_verify`,
  `routing_metrics`, `iteration_checkpoints` (stage tracker, `.error.json`
  record), `flow` (`STOP_STAGES`, `run_routing_pipeline` up to the
  placement stage), the options of `fanout`, `plane`, `escape_assignment`,
  and `bench.unified_overrides`. `PlacementRoutingFeedbackOptions`,
  `EscapePlanOptions` and the `UnifiedRouterOptions` checks route-board
  needs are minimal `PENDING` copies for `rust-m4-global` and
  `rust-m4-unified`.
- `copper route-board`: all 90 options (`allow_abbrev=False`), routing
  profiles, the ERC gate, physicalization, templates, hard macros and every
  option check before the pipeline, with Python's `ROUTING ERROR` lines and
  error records. The pipeline reports not implemented (exit 3) at the
  placement stage (`routeflow.optimize_placement_for_routing`,
  `rust-m4-global`), and so do `--reuse-placement`, typed `--unified-set`
  values, `--debug` tracebacks, Python crashes and error records of error
  classes the Rust errors do not name.
- Corpus: `route-board` (11 boards with `{physical}`), `route-board-options`
  (profiles, stop stage, `--unified-save-inputs`, `--progress`, ...),
  `route-board-errors` and `route-board-mutations`: 54 equal, 26 pending.
  `--unified-time-budget-s` defaults to no budget; a case that sets it would
  be wall-time dependent and stays out of the corpus.

**M4 stream: unified router and `replay-unified` (branch `rust-m4-unified`).**

- `copper-route` (on `rust-m4-global`'s foundation) gained the unified
  router: options (`__post_init__`, `--set`/`--unified-set` overrides),
  `fine_regions`, the uniform lattice (`routing_lattice`, PENDING
  `rust-m4-global`), legality rasters, the native A* kernel (`astar.c`) with
  its heap tie-breaking, net specs and attach candidates (checked,
  extended and local lead-ins, necks), PathFinder negotiation with exact
  proposal conflicts, exact commit, rip-in repair rounds over the shared
  index, plane nets with the pour re-check, `route_smoothing`,
  `plane_estimate` (raster pour estimate), `route_summary`, budgets and the
  congestion export, and `power_islands` (`--power-islands`); plus a typed
  codec for `--unified-save-inputs` states
  and `--save-result` results with Python's `StateError`, `ValueError` and
  uncaught-exception texts.
- `copper replay-unified` is rewritten: every option, error path, summary,
  `--report`, `--save-result`, `-o` and `--progress`.
- Inputs: `tests/rust-port/unified/` (nine `route-board
  --unified-save-inputs` states, malformed and valid variants;
  `generate.py`, README). Corpus: `replay-unified` (states and mutations),
  `-options`, `-errors`, `-malformed`, `-output`: 119 byte-identical, 1
  pending. Replays compare the result state, the report's `unrouted`, the
  KiCad output, exit status and stderr, never the `seconds` or `PROGRESS`
  timing. No case sets a time budget (the default is none); a
  `time_budget_s` deadline reports not implemented.
- Not implemented (exit 3): nested lattices (fine-region patches on ball
  arrays), bus bundles and bus skew tuning (`critical_tuning`; the bus
  phase and its skew notes are rewritten), via technologies, required return vias,
  guide costs, exit reservations, ball-field via sites, ball-grid supply
  islands (`power_islands_ball_grid_only`), `--refanout`, `--exact-escape`, via-in-pad arrays and a dynamically typed
  state value (a text pitch).

**M4 stream: critical and pair routing, `replay-critical` (branch `rust-m4-critical`).**

- `copper replay-critical` is wired (`crates/copper/src/replay_critical.rs`):
  arguments, `load_state` (schema, digest, decode, Python's messages),
  critical inputs dicts and placement checkpoints, the report (critical
  document, DRC report, route summary), `-o` and stdout. `copper-route`
  `critical` ports the stage's guards, `promote_planar_nets`, the job order,
  `--only` selection, hard-macro and asymmetric-rule results, the batch
  intersection check, lane delays (Python `Decimal.sqrt` in `copper-units`),
  fingerprints and documents; `route_summary`, `routing_state` (global route
  decoding, PENDING rust-m4-unified) and the report types of `pair_search`,
  `pair_terminals` and `critical_bundles`. `copper-drc` gained breakout
  relaxation records, now part of the DRC report document.
- Checkpoints of every board with critical rules and derived error inputs
  are under `tests/rust-port/critical/` (`generate.py`, README). The harness
  `json` option takes several key paths and a `without` form, so reports
  carrying wall time compare everything but `seconds` and
  `local_preparation.elapsed_seconds`.
- Routed paths ported: pair terminal preparation (source terminals), the
  coarse aligned-pair and offset-guide candidates with pin stubs and
  checked access lead-ins, budgets, coupled length, atomic DRC validation;
  the joint pair search (`pair_search` collars, departure preferences, the
  resumable A* spine search, resumed frontiers, state escalation;
  `pair_vias` transitions, local via ports and traces) with candidate
  gates and the over-budget fallback; single-ended guides, pin stubs and
  local surface trees (`local_critical`, `surface_path`, `route_style`);
  length-match outcomes within limits; the route summary's plane estimate
  (`plane_estimate`, PENDING rust-m4-unified).
- Corpus (6 `replay-critical*` suites, 90 cases): 81 byte-identical, 9
  not implemented, none different. Not implemented (exit 3): accepted
  search candidates (`pair_refine` spine refinement), return vias
  (`return_paths`), intra-pair skew tuning (`_tune_pair`,
  `pair_tuning_spans`), local contact branches, coarse guide transitions,
  candidate validation on boards with hard macros (copper-pcb
  `validate_hard_macros` for materialized copper), the unified router
  (single-ended cohorts and exact searches; rust-m4-unified), bundles and
  repairs, package-access reservations (`FanoutResult`), plane contacts
  (`plane.stitch_zone_pads`, rust-m4-flow) and length-match tuning over the
  limit.


**M3 stream: the physical editor (branch `rust-m4-editor`).**

- `copper-editor` ports `pcbir.editor` for an opened board session:
  dependency preparation (a preparation mode on the `copper-lang`
  `PackageResolver` that records new pins in memory and writes
  `copper.lock` once), layer selection, physicalization with templates,
  hard macros and fixed cluster poses, the source workspace inputs, and
  the derived scene: components, pads, bodies, silkscreen and board
  artwork, outline paths, mechanical features, the pad-island ratsnest
  (the exact spatial Boruvka MST), net costs, relative-placement
  violations (`copper-drc` `relative_placement_violations` now returns the
  full records), physical feature geometry, source features, the feature
  catalog, dimension source names, macro instance contracts (Python's
  `asdict` reprs) and the routed overlay. Python's macro-copper cache key
  rebuilds the board with only the macro members, which `PhysicalBoard`
  validation rejects whenever another part shares a net; the scene then
  carries that message as `macro_copper_not_materialized`, and the rewrite
  reproduces it.
- `copper edit-mechanical` / `copper edit` with `--scene-output`: four M3
  suites (`edit-mechanical`, `-proxy`, `-macros`, `-options`).
- The loopback HTTP editor: a std-only HTTP/1.0 server with Python's
  Host/Origin/capability checks, bounded strict JSON bodies, the bundled
  assets (embedded from `pcbir/editor/assets`), and the placement
  operations (`lock`, `move`, `move_group`, `apply`, `discard`, `undo`,
  `redo`, `auto_place`, `start_auto_place`, `cancel_auto_place`). The
  harness gained an `http` transcript kind (requests replayed against the
  printed URL, responses recorded as `http.jsonl` with the random port and
  token masked, `placement_job.elapsed_seconds`/`profile` dropped); suite
  `edit-mechanical-http`. Five M3 suites, 280 cases, all byte-identical.
- Browser smokes: `tests/browser/run-editor-smokes.py` runs the binary
  with `COPPER_TEST_EDITOR_BINARY`; `ui`, `keyboard`, `invalid` and
  `artwork` pass against it. `save`, `layout-save`, `curves`, `anchors`
  and `references` need source transactions; `placement` fails on the
  Python package too (its notice assertion is out of date).
- Differences not covered by the corpus: the background placement job runs
  on a thread (a cancelled run's result is discarded when it arrives) and
  `copper-place` reports no phases, so `placement_job.progress` stays at
  `starting`; Python's `profile` (cProfile text) is `null`.
- Not implemented (exit 3): source transactions (`prepare_*`,
  `review_feature(s)`, `save_source`, `discard_source`, `undo_source`,
  `redo_source`, `reload_source`), `open-definition` and definition
  sessions (macro and board_profile documents), via-in-pad via arrays in
  macro previews, and the macro preview of a board whose nets all stay
  inside its macros. The HTTP server exits 3 with the marker on stderr
  when a request reaches one of them.


**M4 stream: the decoupler router (branch `rust-m4-decoupling`).**

- `copper-route` ports `pcbir.decoupling` (`route_decouplers`,
  `improve_decoupling_placement`, `DecouplingRouteResult`),
  `pcbir.decoupling_paths` (the witness dataclasses, `_land`,
  `_return_targets`, `_budget_reason`, `_normal_via`, `_track_ok`,
  `_find_path`, `verify_crosslayer_decoupling`,
  `validate_crosslayer_witness`, `refresh_crosslayer_witnesses`, and
  `propose_crosslayer_decoupling` up to its launch search),
  `pcbir.bypass_links` (`verified_bypass_links`) and
  `pcbir.route_quality` (`escape_paths_cross`). Signatures follow Python's
  keywords in order; `propose_crosslayer_decoupling`'s numeric bounds are
  one `ProposalBounds` value with Python's defaults; progress is an
  optional `FnMut(&str)` stage observer.
- `copper-place` cannot depend on `copper-route` (the reverse dependency
  exists), so the planner exposes `register_decoupling_pass`; `copper`
  registers `copper_route::decoupling::improve_decoupling_placement` at the
  start of `commands::run`. Without a registration (a library user of
  `copper-place` alone) a movable decoupler still exits 3.
- `DecouplingLink` derives `Eq`/`Hash` (sets and witness maps). Python's
  object-identity checks on witness edges (`edge is item`) compare values.
  DRC finding fingerprints are compared as their exact field tuples.
- Suites: the 8 pending `plan-layout-fixtures` and 8 pending
  `route-global-fixtures` six-layer cases are byte-identical; every
  plan-layout* and route-global* suite has 0 pending and 0 different.
- Not implemented (exit 3): new opposite-surface launches
  (`decoupling_paths._connections` and the proposal loop need fanout's
  `_package_candidates`, `_legal_choices`, `_two_leg_candidates`,
  `_interstitial_sites`, plus `access_sites` and `track_containment`), the
  `surface_path_between` detour fallback of `route_decouplers`, and
  via-in-pad arrays (`_source_array_owner`, the materialized-macro
  context's array vias).

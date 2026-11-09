# Architecture and design review (2026-11-09)

A review of the CopperScript workspace at commit `34ac0ab`, asked as "could we
improve usability and speed?". Everything below was measured on this checkout
(4 cores, 15 GB, Rust 1.97.0) or cites a file and line. Measurements in this
section were taken once; treat them as indicative, not as a benchmark.

## Status (2026-11-09, after the first implementation pass)

Implemented on `claude/copperscript-architecture-review-uzigis`:

- **Build loop.** A `fast` profile (release without LTO, 16 codegen units)
  builds the test binaries in about 6 minutes instead of 25. Debug builds
  keep line tables only (the debug test binaries had grown to 25 GB), third-
  party crates are optimised in debug, and plain `cargo build`/`cargo test`
  skip the WebAssembly adapters.
- **Command line.** One option parser for every command (the flag-only,
  report and five private parsers are gone); grouped top-level help with
  summaries; every command's `--help` lists its options and defaults from
  one table, with a test that fails when help and accepted options drift;
  `--version`; `copper completions bash|zsh|fish`; `copper init`; typo
  suggestions; `route-board --list-unified-options`. `clap` was not adopted:
  the shared parser keeps the argparse conventions consumers' scripts rely
  on and adds no dependencies.
- **Workflow.** `route-board` checks plane fills natively by default, so a
  routed board can pass without KiCad (`--verify-plane-fill none` opts out,
  a kicad-cli path still selects KiCad). On the five example ladder boards
  completion, opens and DRC are unchanged; the check costs about 2 s on
  fx10-bridge. `COMPILE ERROR` lines go to stderr, which the Make build no
  longer discards.
- **Language.** Net pin patterns (`U_MODEM.GND*;`) and `source_locations`
  in the IR.
- **Architecture.** One decoder for saved boards (copper-route's duplicate,
  about 1,400 lines, is gone) and a schema registry
  ([schemas.md](schemas.md)) kept in step with the sources by a test.

Corrections to the findings below:

- `decouples = U.VDD;` (bare) was already accepted and compiles to the same
  IR as the quoted form; only the documentation showed the string form.
- The two `OnceLock` hook registries are not silent: without
  `install_hooks` both report "not implemented" naming the missing call.
  Replacing them with parameters would touch about 27 call sites for little
  gain, so they stay.

Not done, and why:

- **Splitting `copper-route`.** The pipeline layer is not a leaf:
  `critical`, `detailed`, `unified_router/route`, `decoupling_paths` and
  `state_codec` import it. A split needs those back-edges cut first (for
  example, moving the congestion-feedback and checkpoint types they use
  into a lower module); that is a design change, not a move.
- **Speed items** (inventory hashing, conflict-index rebuilds, hasher,
  DRC broad phase, parallel trials) were not in this pass's scope.
- **Python byte-exactness, a `state/v2` format and `serde`.** These change
  consumer-visible outputs and frozen fixtures, so they need an owner
  decision first.
- **One workspace error type, one DRC, test reorganisation, CI caching.**
  Each is a larger change of its own; CI changes were left alone because
  GitHub Actions cannot be run from this environment.
- **`copper build`** (a native replacement for `make/route-build.sh`) and
  repetition constructs beyond pin patterns.

## Summary

CopperScript is in better shape than its size suggests. The compiler front end
is fast (ERC on an example board in 12–80 ms), diagnostics carry codes,
positions, hints and machine-applicable fixes, the dependency surface is tiny
(27 third-party packages), `unsafe` is forbidden and hash-order independence
is enforced by clippy. The two example boards that have ground planes route to
a passing result in 1.4 s (nrf52) and 18 s (full-vertical) when the built-in
plane checker is used.

The problems are concentrated in four places:

1. **A hidden fixed cost dominates small runs.** 30 % of all instructions in
   the nrf52 route are SHA-256, most of it hashing the entire CopperLib
   checkout (12,495 files, 122 MB) during footprint resolution, and the rest
   computing copper identities inside the router's conflict detection.
2. **The default pipeline depends on KiCad to say "pass".** Without
   `--verify-plane-fill native` a fully routed board reports `routing=fail`,
   and the CI ladder's example boards are configured so that they can never
   pass. The native checker exists and works; it is just not the default.
3. **The command line hides itself.** `copper --help` is an unordered list of
   39 names with no descriptions; `copper route-board --help` is three lines
   for a command with about 100 options and an 81-key `--unified-set`
   escape hatch; there is no `--version`, no `init`, no `fmt`, no completions.
4. **`copper-route` is half the codebase** (106 K of 216 K lines, 84 public
   modules, four files over 4,000 lines) and is where every change lands.
   Three hand-rolled state codecs, two DRCs, two board models and a
   Python-compatibility layer with no remaining oracle add maintenance drag
   without buying speed.

Each is fixable incrementally; a suggested order is at the end.

## Measurements

| What | Result |
| --- | --- |
| `cargo build --release -p copper`, cold | 4 m 30 s wall, 9 m 25 s CPU |
| Slowest compile units | copper-route 132 s, copper binary 103 s, ems 38 s, copper-lang 31 s |
| `cargo test --workspace --release --no-run`, after the binary build | 25 m 29 s for 125 test binaries |
| `cargo test --workspace --release`, run only | 34 s; 1,484 passed, 0 failed, 6 ignored |
| `copper check` (valid_board, offline) | 34 ms |
| `copper check` (nrf52, full_vertical, offline) | 74 ms, 79 ms |
| `route-board` nrf52 (ladder args) | 1.36 s, status `fail`, 19/19 nets routed |
| `route-board` nrf52 + `--verify-plane-fill native` | 1.36 s, status `pass`, native DRC pass |
| `route-board` full-vertical (ladder args, 3 candidates) | 14.6 s, status `fail`, 50/50 nets routed |
| `route-board` full-vertical + native checker | 17.8 s, status `pass`, requirements blocked |
| CPU utilisation during routing | user ≈ real: one core |
| `bench/ladder-baseline.json` (CI, 2026-10-30) | 7 of 9 boards `route_status: fail`, 1 skipped, `phases: {}` everywhere |

The callgrind profile of the nrf52 route (19.65 G instructions):

| Share | Where |
| --- | --- |
| 30.5 % | `sha2::sha256::compress256` |
| 29.2 % (inclusive) | `PackageResolver::resolve` → `module_inventory` (hashing every file of the required module) |
| 14.6 % (inclusive) | `unified_router::astar::search_outcome` (the actual A* kernel) |
| 5.2 % | `astar::heap_pop` |
| ~10 % | `malloc`/`free`/`memcpy` |
| ~7 % | SipHash (`IndexMap`'s default `RandomState` hasher) |
| ~3 % | `num_bigint` gcd/shift (rational geometry) |
| ~2.5 % | `format!`/`fmt::write` |

## Speed

### Fixed costs before routing starts

**Module inventory hashing.** `PackageResolver::resolve`
(`crates/copper-lang/src/packages.rs:1019`) verifies a required module by
re-reading and SHA-256-hashing every file under it (`module_inventory`,
`:371-407`). The CopperLib cache holds 12,495 files / 122 MB, so this costs
roughly 0.4–0.5 s per process even when the lock already carries every
file's hash. `FootprintResolver` memoises the result per instance
(`footprints.rs:98-105`), but `route-board` constructs resolvers in several
stages. Options, in order of effort:

- Cache the verified inventory per process behind the resolver (one
  `OnceCell` per module path and version).
- Persist a verified-inventory stamp next to the cache (lock checksum plus
  file sizes and mtimes); re-hash only when a stamp mismatches. The lock
  already records size and SHA-256 per file (`LockedFile`), so a size +
  mtime check is a safe fast path with full hashing as the fallback.
- Hash only the assets the board actually references under `--offline`;
  keep whole-module verification for `copper lock` and the first online
  resolve.

Expected effect on small boards: 25–30 % of wall-clock. On CopperProbe-size
runs it is noise, but every `check`, `compile`, LSP analysis and editor
session that resolves footprints pays it.

### Routing kernels

The exact-arithmetic design is not the bottleneck. The A* kernel is f64 costs
and i32 arrays with a hand-rolled heap (`unified_router/astar.rs:24-79`),
the narrow-phase clearance test is i128 with a bounds prefilter
(`copper-physical/src/geometry.rs:434-440`), and `Decimal`/`Fraction` do not
appear in `unified_router/{astar,search,rasters,router}.rs` or
`routing_clearance.rs`. The time goes to rebuilding and re-hashing, not to
arithmetic:

1. **Conflict index rebuilt per negotiation iteration.** `conflicts()`
   constructs a fresh `RoutingClearanceIndex::new(self.board)` each
   iteration (`unified_router/negotiate.rs:64`), re-transforming every pad
   and re-adding all board copper (`routing_clearance.rs:458-550`), then
   computes a `copper_identity` for every blocker record, which is a
   canonical JSON dump plus SHA-256 (`routing_clearance.rs:169-206, 1219`).
   This is the second SHA-256 hot spot. Fix: keep one index and use the
   existing `remove(owner)` (`:935`) for the nets that changed; correlate
   conflicts by the already-unique `occurrence` token and compute identity
   strings lazily for reports.
2. **Rasters rebuilt per `Router::new`.** Outline material and edge
   distance are recomputed for every node and per width class
   (`unified_router/rasters.rs:77-151, 307-392`) each time a router is built:
   twice per repair round (`route.rs:579, 589`), again for `recheck_pours`,
   and once per congestion-feedback trial. `docs/routing-performance.md:283-290`
   records this as 715–1,016 s of setup on CopperProbe. Rasters are pure
   functions of lattice signature, outline and immutable copper, so caching
   them and rasterising only the delta is provably output-neutral.
3. **O(n²) DRC spacing sweep.** `check_copper_spacing`
   (`copper-drc/src/lib.rs:1054-1156`) compares every track and via pair
   with no broad phase and runs per tail, inside every
   `materialize_hard_macros` and two or three more times in
   `route_board.rs`. Binning with the scheme `routing_clearance.rs:1468-1474`
   already uses, then sorting findings by `(left, right)` index, keeps order
   and removes a documented bottleneck.
4. **Full-plane copies per A\* call.** `via_owner()` returns `.to_vec()`
   of a plane-sized raster per search (`unified_router/search.rs:696, 734-741`),
   `via_transitions` copies one per layer pair (`:822-842`) and
   `ignore_soft_costs` allocates two full-size vectors (`:925`). The same
   fix was already applied to track rasters (`:930-932`); borrow slices.
5. **Route-map clones per iteration.** `self.routes.clone()` and best
   snapshots (`negotiate.rs:250, 278, 348`) copy four `IndexSet<i64>` per net
   per iteration. Pass by reference and snapshot only changed nets.
6. **String-keyed global router.** `usage`/`history` are
   `IndexMap<String, i64>` hashed per edge relaxation
   (`routing.rs:1096-1097, 1375-1376`) and `search_steps` clones a `Vec` with
   a `String` resource id per expansion (`:517-546`), multiplied by 20 global
   iterations × candidates × up to 96 movement trials. Dense ids fix this and
   are already the pattern in the unified router.
7. **Hasher choice.** `IndexMap` with the default `RandomState` is 7 % of
   the profile. Order in an `IndexMap` is insertion order, not hash order, so
   swapping in a fast deterministic hasher (FxHash or ahash with a fixed
   seed) is output-neutral and cheap.
8. **Parallelism.** None exists outside `copper bench --jobs`. Per-net
   threading inside negotiation is correctly ruled out
   (`docs/routing-performance.md:1367-1382`), but placement movement trials
   (`routeflow.rs:174-204`) and screened congestion proposals
   (`congestion_feedback.rs:1218-1247`) each route a cloned board and select
   by `(score, fingerprint)`. Evaluating them on scoped threads and selecting
   in the original order is deterministic. Blockers: `Graph.step_cache` is a
   `RefCell` (`routing.rs:504`) and the offsets cache is `thread_local!`
   (`search.rs:27-30`).

### External process cost

`--verify-plane-fill <kicad-cli>` spawns `kicad-cli --version` and
`kicad-cli pcb drc --refill-zones --save-board` once initially, once per
filled-copper trial (default 8) and again for cleanup
(`route_board.rs:146, 1348, 1543`; `plane_verify.rs:612, 659`): 2 to ~20
processes, historically 33–41 s per phase (`routing-performance.md:1149`).
`--verify-plane-fill native` does the same in-process with `copper-cam`'s
fill and DRC and passed both example boards here. Making `native` the
default (and KiCad the opt-in cross-check) removes the install dependency,
the process spawns and the "fail without KiCad" result at once.

### Build and test loop

- Release build: 4 m 30 s cold. `copper-route` (132 s) and the `copper`
  binary (103 s) are the critical path; the workspace-wide
  `codegen-units = 1` + `lto = "thin"` make every incremental rebuild of the
  binary pay for LTO. A `[profile.ci]` or a dev-facing profile without LTO,
  and `default-members` that exclude the two WASM crates (which pull
  wasm-bindgen, syn and proc-macro2 into every native build), would shorten
  the edit–run loop without touching the release artefact.
- Test build: compiling the 125 test binaries in release took 25 m 29 s
  *after* the `copper` binary was already built, while running them takes
  34 s. The `[profile.release]` settings (`codegen-units = 1`, `lto =
  "thin"`) apply to every test binary, and 23 CLI test files each link the
  whole binary. This is the single largest cost in the contributor loop
  and is why CI falls back to a debug `cargo test`. A `[profile.test]`
  without LTO and with `codegen-units = 16`, plus one shared test harness
  per crate instead of one binary per `tests/*.rs` file, should bring the
  test build under five minutes without changing what is tested.
- CI compiles the workspace four times (clippy debug `--all-targets`,
  release build, debug `cargo test`, `cargo install` in the Make job) with
  no cargo cache step (`.github/workflows/rust.yml:71-117`). README says
  `cargo test --release`; CI runs debug. Add `[profile.test] opt-level = 1`
  (or `[profile.dev.package."*"] opt-level = 2`), cache `target/`, and reuse
  the release binary for the CLI tests.
- The ladder baseline has `"phases": {}` for every board, so the one place
  that could show where time goes shows nothing. Emit per-stage wall-clock
  into the report (the pipeline already names its stages) and keep the
  baseline's phase table filled.

## Usability

### Command line

- `copper --help` prints 39 command names in registration order with no
  descriptions (`crates/copper/src/commands.rs:18-58, 74-79`): `compile-macro`
  first, `check` eighteenth.
- Per-command help is one usage line and one sentence. `route-board --help`
  is three lines; the command has about 100 distinct `--flags`
  (`route_board.rs:166-258`), 18 of them `--unified-*`, plus `--unified-set
  key=value` with 81 keys that appear nowhere in help or user docs
  (`unified_router/options.rs`). `replay-unified` spells the same thing
  `--set`.
- Four hand-rolled argument parsers with different rules: `args.rs` allows
  abbreviations, `option_parser.rs` has `parse` and `parse_exact`,
  `report_args.rs` treats an unknown dashed argument containing a space as a
  positional (`:113`), and `check.rs:38-66` / `compile.rs:24-43` loop by hand.
  No `--version` exists, yet `make/route-build.sh:125` records
  `"$compiler" --version` into `run.json`, so provenance stores nothing.
- Exit codes: 0, 1 (ERC fail), 2 (usage *and* compile *and* routing error),
  3 (not implemented). Compile errors print to stdout as `COMPILE ERROR:`
  (`check.rs:121`, `route_board.rs:766`). `--json` exists on 10 of 38
  commands.
- Aliases and near-duplicates: `edit`/`edit-mechanical`,
  `bench`/`benchmark-routing`, `export-kicad`/`export-kicad-pcb`.

Recommendation: adopt `clap` (derive) once. It yields grouped, complete help,
`--version`, shell completions, typed values, consistent abbreviation rules
and a single place to document every option; the retired-flag migration
messages (`route_board.rs:56-92`) can be kept as `clap` hidden aliases with
custom errors. Generate `docs/cli-workflow.md`'s option tables from the same
definitions so docs cannot drift (today 68 of 85 route-board options are
documented, and `cli-workflow.md:79-87` still describes a flag the same file's
table and the binary reject).

### Workflow

From a fresh checkout to a routed board: install Rust, KiCad 10, GNU Make,
bash, `jq`, `sha256sum`; write `copper.mod` with one `footprint-library`
line per namespace (18 lines in the root manifest); `copper lock`; one online
`copper check --locked`; then `make route`, which runs `check
--macro-inputs`, `route-board --offline`, `kicad-cli pcb drc`, a `jq` gate
and writes `run.json` (`make/route-build.sh:80-170`). Users manage
`build/<board>/runs/<UTC>-pid/`, `.copper-cache`, `--checkpoint-dir` and a
`RUN_DIR` that must be new and empty with no clean target
(`make/board.mk:51-53`). Make exits 2 for any failure and the docs say to
open `run.json` for the real code (`docs/make-builds.md:94-95`).

Recommendations:

- `copper build` (or `copper route` with `--routing-profile`) that performs
  what `route-build.sh` does natively, defaults to the native plane checker,
  prints its step plan and detects KiCad as an optional cross-check.
- `copper init [--template nrf52]` writing `copper.mod`, `board.copper`,
  `.gitignore` and the Make include; `copper runs list|clean`.
- Project-level defaults for `--locked --offline` (a `[defaults]` block in
  `copper.mod`) instead of repeating them on every call.
- Ladder: give the example boards `"native": true` so they can pass, and fail
  the CI step on a completion regression, not only on error/timeout
  (`rust.yml:232-236`).

### Language

The front end is good: `copper check --json` carries severity, code, subject,
hint, ranges and `fix.replacement`; `copper explain CODE` works; the LSP has
diagnostics, completion, hover, definition, symbols and quick fixes. The
costs are in repetition and one inconsistency:

- No repetition constructs. `nrf52_coin_cell/board.copper:98-118` has 21
  near-identical `constraint fixed_placement(...)` blocks;
  `full_vertical/board.copper:172-195` lists 24 `U_MODEM.GND_nn;` endpoints;
  `fx10_bridge/board.copper:21-33` repeats 13 decoupling capacitors that
  differ in one field. Pin globs (`U_MODEM.GND_*`), inline poses on
  `component`, and a component-array or `for` form would remove most of it.
- The same binding is stated three times in `valid_board/board.copper`
  (`configure` at `:30-33`, `net` at `:54-58`, `interface bind` at `:95-98`);
  nets and supplies of the same name are declared twice and every
  `power_state` re-lists every supply. Derive the interface binding from
  `configure` and let `supply` attach to a `net`.
- `decouples = "U_MCU.VDD"` is a quoted string (README:34,
  `language-reference.md:10`) while the identical reference is a bare
  identifier inside `net { }`. The LSP therefore cannot navigate or rename
  it. Accept bare `U.VDD` in `decouples` and macro mappings.
- The compiled IR records no source spans, so CopperReview has to grep
  sources to find a declaration (CopperReview README: "since the IR does not
  record source lines yet"). `copper-lang` already has `Span` and `Location`;
  emitting them per component, net and constraint is cheap and unlocks
  cross-tool navigation.
- `docs/` holds 76 files with plans, backlogs and research interleaved with
  references; `language-reference.md` (2,134 lines) opens with the
  decoupling role and reaches `board {}` at line 114. Split `docs/user/` from
  `docs/contributing/`, move plans out of the index and reorder the
  reference lexical → board → components → nets → constraints.

## Architecture

### Crate graph

```
copper-json → copper-units → copper-state, copper-physical → copper-lang
copper-lang → copper-pcb (also imports lang: paths, stackup, silkscreen, macros)
copper-pcb  → copper-drc, copper-report, copper-antenna
copper-cam  → copper-assembly, copper-panel, copper-route
copper-drc  → copper-place → copper-editor, copper-route
copper-route → copper-web-route, copper
hidden back-edges: copper-place ◄╶ route (planner.rs:70-75 OnceLock registry)
                   copper-drc   ◄╶ route (copper-drc/src/macros.rs:29)
```

Issues, in order of drag:

1. **`copper-route` is a god crate.** 106,587 lines (49 %), 84 `pub mod`s
   (`lib.rs:36-117`), 620 `pub fn` against 176 `pub(crate) fn`, 99
   `#[allow(clippy::too_many_arguments)]` workspace-wide. Longest functions:
   `finish` in `copper/src/route_board.rs:1340-2086` (746 lines),
   `route_critical_nets` (`critical.rs:965-1605`, 640), `route_unified`
   (`unified_router/route.rs:640-1172`, 532), `encode_board`
   (`state_codec.rs:3123-3651`, 528). Natural seams by module prefix:
   `-core` (clearance, lattice, costs, unified_router, global routing, layer
   assignment), `-critical` (critical*, pair_*, rf_geometry, return_paths),
   `-escape` (escape_*, fanout*, bga_fanout, pin_access, via_lattice),
   `-planes` (plane*, power_islands, pad_stitch, filled_*, decoupling*) and
   `-flow` (flow, routeflow, congestion_feedback, checkpoints, state_codec,
   bench/ladder, reports). Start with `-flow`: it is leaf-side and `flow.rs`
   exposes only three `pub fn`.
2. **Three state codecs (6,299 lines).** `copper-state/src/lib.rs` (390),
   `copper-pcb/src/state_codec.rs` (1,435, decode-only) and
   `copper-route/src/state_codec.rs` (4,474) re-implement `footprint_pad`,
   `placement`, `design_rules`, `via`, `physical_board`; 38 of copper-pcb's
   70 function names recur. The split exists because the two use different
   error types. One `StateCodec` trait beside each type and one error type
   deletes the copy.
3. **Python-compatibility layer without an oracle.** About 3,500 lines
   exist only to reproduce Python byte-for-byte: zlib-exact DEFLATE
   (`copper-assembly/src/deflate.rs`, 937), `Decimal` emulation
   (`copper-units/src/decimal.rs`, 649), JSON escaping tables, `difflib`,
   `pypath`, `pyfloat`, LSP error codes chosen by Python exception class
   (`copper-lsp/src/pyerr.rs:33-36`), and state files that tag types by
   Python module path (`pcbir.physical:PhysicalBoard`,
   `copper-state/src/lib.rs:35-37`). The fixtures these guard are frozen and
   "their generators were removed" (`crates/README.md:74`), so the
   exactness is now self-referential. It also blocks `serde` (which would
   replace most of item 2), `clap` and faster hashing. Incremental path:
   declare Rust the reference, keep the v1 *reader* frozen, add a `state/v2`
   writer with Rust type names via `serde`, regenerate fixtures from Rust
   with a reasoned commit.
4. **String-typed, duplicated error handling.** 21 error types, 8 of
   which implement `std::error::Error`; `Result<_, String>` 207 times;
   `copper_pcb::Error` and `copper_route::unified_error::Error` both model
   `Value | NotImplemented`. About 900 `expect`/`unwrap` in non-test code
   (copper-route 613). No panic on malformed input was found, and panics
   surface as tracebacks via `resume_unwind` (`main.rs:58-69`), but 218
   `NotImplemented` sites are user-reachable dead ends. One workspace error
   type with a source location, and `Error::Crash` for data-dependent
   invariants, is the cheap version.
5. **Layering inversions.** `copper-pcb` (the "backend-neutral" physical IR,
   CS-039) imports `copper-lang` in 10 files; `copper-route → copper-cam` is
   7 lines in one file; the two `OnceLock` registries mean `copper-place`
   silently misbehaves in any binary that forgets
   `copper_route::install_hooks()` (`copper-route/src/lib.rs:130-141`). Move
   stackup/silkscreen/paths into `copper-physical` and pass the decoupling
   pass and array materialiser as `&dyn` parameters.
6. **Two board models, two DRCs.** `copper-drc` (exact rational geometry on
   `PhysicalBoard`) and `copper-cam/src/drc.rs` (f64 and `i_overlay` on a
   board re-parsed from `.kicad_pcb`) both check clearance, hole-to-hole,
   annular ring and edge; `copper-assembly` and `copper-panel` consume the
   cam model. 34 digest/fingerprint functions in 22 files with 17 independent
   `Sha256::new()` sites. Make `copper-cam` consume `PhysicalBoard` plus fill
   polygons and route all hashing through one digest module.
7. **Test organisation.** 1,490 tests, 69 % in copper-route across three
   conventions (`tests/` 15 K lines, `src/*_tests.rs` 7 K lines, inline),
   with an `extern crate self as copper_route` hack (`lib.rs:122-128`). 23
   CLI test files spawn the binary. Unify on `tests/` with one dev-only
   `copper-testkit` crate.
8. **Schema versioning is scattered.** 30 distinct schema literals
   (`copperscript-ir/v0.1`, `copperscript-route-board/v0.1`,
   `assembly-lock/v0.2` and `/v0.4`, ...), no central constants module, no
   JSON Schema files, no compatibility test. Consumers couple through Make
   targets and 20–30 pinned CLI flags per board (`bench/ladder.json`). A
   `copper_schemas` module and a documented stable-flag table would make the
   consumer contract explicit.

### What to keep

The determinism rules (no `HashMap`, `IndexMap`/`BTreeMap` only, LF output),
`unsafe_code = "forbid"`, exact-pinned unstable dependencies, `--locked`
resolution with per-asset hashes, the decision log (CS-001…CS-143), the
routing rebuild's diagnosis of accretion (in Git history),
the diagnostic design (codes, ranges, fixes, `explain`), and the
`issues`/`view` iteration loop are all worth preserving through any of the
changes above.

## Suggested order

Each step is independently shippable and measurable with the ladder.

1. **Default to the native plane checker** and give the ladder's example
   boards `"native": true`. One day. Turns "fail without KiCad" into pass,
   removes the external process cost, and makes the baseline meaningful.
2. **Cache module inventory verification** (per process, then stamped on
   disk). One to two days. About 30 % off every small run and every
   footprint-resolving command.
3. **Stop rebuilding the conflict index per iteration and drop SHA-256
   identities from the hot path**; borrow via rasters instead of copying.
   Three to five days. The second large constant factor, output-neutral.
4. **Fast deterministic hasher for `IndexMap`**, broad phase for
   `check_copper_spacing`, cached rasters. One to two weeks together.
5. **`clap` migration with generated option docs, `--version`,
   completions, `copper init`, `copper build`.** One to two weeks. The
   largest usability win per hour.
6. **Emit source spans in the IR**; accept bare pin references in
   `decouples`; add pin globs. One week. Unlocks CopperReview navigation and
   cuts board source size.
7. **Split `copper-route-flow` out of `copper-route`**, unify the state
   codec on `serde` behind a `state/v2` writer, one error type. Several
   weeks, and the step that pays back on every later change by shortening
   the build and making seams explicit.
8. **Parallel movement and congestion trials** once the `RefCell` and
   `thread_local!` caches are made per-thread. After the above; the ladder
   decides whether it is worth keeping.

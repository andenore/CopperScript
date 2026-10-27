# Rust workspace

The Rust rewrite of CopperScript ([plan](../docs/rust-port-plan.md)). The
Python CLI stays the reference implementation until cutover; nothing here
changes its behaviour.

| Crate | Contents |
|---|---|
| `copper-json` | JSON values and a serializer that reproduces Python's `json.dumps` byte for byte; Python text-mode newlines |
| `copper-units` | `Decimal` with Python's default context, integer nanometres (`Nm`), `Fraction`, quantities and the `100 nF` formatter |
| `copper-state` | The frozen `copperscript.state/v1` format: read, verify, write |
| `copper-physical` | Exact board geometry (outlines, cutouts, holes, slots, line/arc paths), footprints and the `.kicad_mod` importer |
| `copper-lang` | Lexer, parser, package resolution, lowering (electrical, mechanical, silkscreen), ERC, the compiled JSON and the KiCad schematic backend |
| `copper-pcb` | Physical board IR, physicalization (proxy and resolved footprints), placement templates, hard-macro scenes and the KiCad PCB/project backend (M2, in progress) |
| `copper-drc` | Physical DRC findings and coverage, exact copper connectivity, breakout spacing, placement legality and hard-macro materialization (M2 subset of M3's DRC) |
| `copper-report` | Run diagnostics and reports: diagnostic explanations, a run's issues grouped by cause, board window views (scene reader, legend and SVG) and engineering summaries (M5) |
| `copper` | The `copper` binary; implemented commands: `agent-guide`, `state` (M0); `check`, `compile`, `query`, `power-check`, `audit-footprints`, `lock`, `compile-macro`, `library` (M1); `check-footprint`, `export-kicad`, `export-kicad-pcb` (M2) |

## Build and test

```sh
cargo build --release          # target/release/copper
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

## Compare with the Python CLI

The harness runs every corpus case on both implementations and compares exit
status, stdout, stderr and output files byte for byte. It needs the Python
package installed (`uv sync --locked` or `pip install -e .`).

```sh
python scripts/compare_implementations.py                 # the gated suites; exit 1 on any difference
python scripts/compare_implementations.py --list --suite all
python scripts/compare_implementations.py --suite check --keep build/compare/keep
python scripts/compare_implementations.py --jobs 4                      # parallel; timings only comparable with --jobs 1
python scripts/compare_implementations.py --suite ladder --oracle-when-pending --report build/compare/ladder.json
```

- The corpus is `scripts/compare_corpus.json`. Suites up to its
  `accepted_milestone` are gated. Raise it when a milestone is accepted.
- An unimplemented Rust command exits 3 with "not implemented in the Rust
  rewrite" on stderr; the harness reports it as `pending`.
- A difference that is accepted on review goes in
  [the differences log](../docs/rust-port-differences.md) with both result digests.
- The `ladder` suite expands `bench/ladder.json`; its report records the run
  time of both implementations per board.

## Oracle fixtures

Unit tests check the crates against fixtures recorded from the Python oracle:
decimal arithmetic, quantity formatting, float `repr`, JSON escaping, real
state files, and parse outcomes (tree digest or exact diagnostic) for every
in-repo source and seeded mutations of each. `src/agent_guide_topics.json` in the binary is the guide text,
copied the same way.

```sh
python scripts/generate_rust_fixtures.py          # after a deliberate oracle change
python scripts/generate_rust_fixtures.py --check  # CI
python scripts/generate_rust_fixtures.py --dump-parse examples/valid_board/board.copper
python scripts/generate_rust_fixtures.py --parse-corpus ../CopperLib --output build/copperlib-parse.json
COPPER_PARSE_FIXTURE=build/copperlib-parse.json cargo test -p copper-lang --test python_oracle
```

## Rules

- Output must not depend on hash order: `HashMap` and `HashSet` are
  disallowed by clippy. Use `IndexMap` or `BTreeMap`.
- Text output goes through `copper_json::text` and uses LF on every platform;
  the harness reads Python's Windows CRLF as LF.
- Lengths are `Nm` (`i64`); exact values use `Decimal` or `Fraction`, never
  `f64`, unless the Python code uses a float.

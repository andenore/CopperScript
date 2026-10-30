# Rust workspace

The CopperScript implementation: the `copper` binary and the crates behind it.
Install the binary with `cargo install --locked --path crates/copper`.

| Crate | Contents |
|---|---|
| `copper-json` | JSON values and a serializer that reproduces Python's `json.dumps` byte for byte; Python text-mode newlines |
| `copper-units` | `Decimal` with Python's default context, integer nanometres (`Nm`), `Fraction`, quantities and the `100 nF` formatter |
| `copper-state` | The frozen `copperscript.state/v1` format: read, verify, write |
| `copper-physical` | Exact board geometry (outlines, cutouts, holes, slots, line/arc paths), footprints and the `.kicad_mod` importer |
| `copper-lang` | Lexer, parser, package resolution, lowering (electrical, mechanical, silkscreen), ERC, the compiled JSON and the KiCad schematic backend |
| `copper-pcb` | Physical board IR, physicalization (proxy and resolved footprints), placement templates, hard-macro scenes and the KiCad PCB/project backend |
| `copper-drc` | Physical DRC findings and coverage, exact copper connectivity, breakout spacing, placement legality and hard-macro materialization; signal-integrity screening (`si-check`) |
| `copper-lsp` | The language server (`copper lsp`): Content-Length framing, JSON-RPC dispatch with Python's error codes and messages, the debounced diagnostics worker, recovering parse, symbols, hover, definition, completion, compiler-backed diagnostics and quick fixes |
| `copper-report` | Run diagnostics and reports: diagnostic explanations, a run's issues grouped by cause, board window views (scene reader, legend and SVG) and engineering summaries |
| `copper-assembly` | Assembly locks and checks, BOM/CPL and hand-assembly/ordering CSVs, and native KiCad manufacturing files (`zipfile`-exact archives with a zlib-exact DEFLATE) |
| `copper-place` | The placement planner behind `plan-layout`: seeds, legalization, rigid clusters, refinement, escape spacing, candidate selection, requirement verification and the engineering summary |
| `copper-route` | Routing: exact clearance index, search lattices, the negotiated-congestion global router, layer assignment, the placement/routing feedback controller behind `route-global`, escape planning, the decoupler router (`route_decouplers`, cross-layer bypass witnesses, the planner's registered decoupling-access pass); the unified detailed router, fine-region planning, power islands, the typed `--unified-save-inputs` state codec; the `route-board` command line and the full routing pipeline (escape and bypass reservations, fanout, plane stitch, congestion-driven placement feedback, kicad-cli plane verification), the `bench` ladder runner, `benchmark-routing` acceptance cases and report helpers |
| `copper-editor` | The physical editor: the routed editor overlay (`editor-overlay`), and `edit-mechanical`'s board and definition (macro, board_profile) sessions, derived scene, pad-island ratsnest (exact Boruvka MST), artwork and physical feature inventory, reviewed source transactions (token patches, `difflib`-exact diffs, locked atomic saves) and the loopback HTTP editor |
| `copper` | The `copper` binary: command dispatch and option parsing for every command (`copper --help` lists them) |

## Build and test

```sh
cargo build --release -p copper     # target/release/copper
cargo fmt --all --check
cargo clippy --workspace --all-targets --release -- -D warnings
# Fill the package caches once, so the tests run offline:
for board in examples/*/board.copper; do
  target/release/copper check "$board" --locked
  target/release/copper audit-footprints "$board" --locked
done
cargo test --workspace --release
```

## Golden fixtures

Some tests compare the crates against fixtures recorded from the former Python
implementation (last Python commit `b16122a`): decimal arithmetic, quantity
formatting, float `repr`, JSON escaping, real state files, parse outcomes (tree
digest or exact diagnostic) for in-repo sources and seeded mutations of each,
search lattices, escape plans, critical bundles and pair tuning. They live in
`crates/*/tests/fixtures/` and `tests/rust-port/critical/`, with the sources
they parse under `tests/rust-port/` and `examples/`.

These fixtures are frozen: their generators were removed with the Python code.
A deliberate behaviour change updates the affected expectation by hand, with
the reason in the commit message. `src/agent_guide_topics.json` in the binary
is the guide text; edit it in place.

## Rules

- Output must not depend on hash order: `HashMap` and `HashSet` are
  disallowed by clippy. Use `IndexMap` or `BTreeMap`.
- Text output goes through `copper_json::text` and uses LF on every platform.
- Lengths are `Nm` (`i64`); exact values use `Decimal` or `Fraction`, never
  `f64`, unless the former Python behaviour used a float (outputs keep
  Python-compatible numerics such as `pyfloat` and `json.dumps` formatting).

# Live compiler prototype

Edit a self-contained `.copper` sensor circuit, introduce an I²C pull-up fault,
raise the supply voltage or misspell a pin. The same `copper-lang` compiler,
ERC, located diagnostics and net queries used by the CLI run in WebAssembly
inside a dedicated browser worker. No server executes user source.

## Build and preview

From the repository root, with its pinned Rust toolchain and Node 22 or newer:

```console
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.130
node scripts/build_demo.cjs
node scripts/check_demo.cjs
node scripts/check_site.cjs
node scripts/serve_demo.cjs
```

Open `http://127.0.0.1:4173/CopperScript/demo/`. The preview server also serves
`/demo/`, but the prefixed path exercises GitHub Pages project hosting.
`PORT` changes the local server port. Stop the server with Ctrl+C.

The bindings CLI must match the crate's exact `wasm-bindgen` version. An
official prebuilt CLI can be used instead of compiling it: set `WASM_BINDGEN`
to its executable path. The build reads Cargo metadata to respect the configured
target directory, then writes ignored assets to `site/demo/pkg/`. Do not hand
edit generated bindings. The Pages workflow builds them from the same source
revision it publishes.

Optional native parity check, after `cargo build --release -p copper`:

```console
node scripts/check_demo.cjs target/release/copper
```

Use `target/release/copper.exe` on Windows. Rust adapter tests are included in
`cargo test --workspace --release`; run them alone with
`cargo test --release -p copper-web`.

## Contract and boundaries

- `copper-web::analyze` accepts source and returns `copperscript.playground/v1`
  JSON containing the existing `copperscript.check/v1` document and native
  component, pin and net queries. Decimal quantity strings retain native
  semantics. The frontend renders text with DOM text nodes, never source HTML.
- The example uses the existing `tiny` demonstration library. It makes no
  manufacturer-backed component or hardware-function claim.
- Package imports are rejected with a located `WEB_IMPORT` finding before
  package resolution. GitHub project loading and browser dependency resolution
  are future work. No footprint fetching, physicalization, placement, routing,
  native verification, manufacturing export or qualification runs here.
- The connection view is derived from the compiled electrical design. Its
  layout is a connectivity diagram, not physical board geometry or routed copper.
- Input is limited to 32 KiB; browser analysis has a 5-second deadline and
  runtime loading a 20-second deadline. Stop or a WASM trap terminates the
  worker. Check circuit restarts it with the current source preserved.
- Every edit increments the document revision and clears diagnostics and
  connections immediately. Late results from older revisions are discarded.
  Typing triggers analysis after 450 ms; Ctrl/Cmd+Enter checks immediately.
- Clicking findings and pins selects source. Compiler columns count Unicode
  code points; navigation converts them to textarea UTF-16 offsets. Quick fixes
  use the compiler's machine-applicable ranges and replacements.
- All assets are served from this site; there are no analytics or outbound
  source requests. Viewing the page still requests ordinary static assets.
- Warm timing displayed in the UI measures analysis and JSON generation in
  the worker. It excludes module loading, message transfer and DOM rendering;
  it is not a native-versus-WASM performance benchmark.

## Meaningful checks

`check_demo.cjs` instantiates the actual shipped WASM in Node. It checks the
healthy example, pull-up fault and changed net members, voltage fault, typo
and suggested fix, syntax failure, import/size limits, Unicode/CRLF locations
and recovery after failures. It also loads the actual worker module with Node
message/fetch adapters and checks revision IDs and syntax-error recovery. This
is a worker protocol test, not a browser UI test. An optional native binary
checks CLI agreement.

For browser acceptance, exercise each guided change, Apply suggested fix,
diagnostic/pin source selection, syntax-error recovery, stale results during
rapid edits, Stop/restart, Download source, keyboard access and widths down to
320 px. Verify the module and worker load from `/CopperScript/demo/`. Missing
or blocked WASM must produce a visible error and preserve the editor buffer.

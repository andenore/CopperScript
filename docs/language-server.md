# CopperScript language server

Macro definitions and package-exported macro bindings share the source frontend.
The editor indexes named members/tracks/net roles, offers declaration/property
and binding-role completion, and navigates `alias.MacroName` to its source.
Standalone definitions receive structural/type diagnostics without preparing
footprints; board-bound definitions inspect footprints locked/offline and report
unavailable contracts. Unsaved definition buffers feed board diagnostics through
the same source-reader hook. Editor analysis never writes generated macro assets.

The LSP belongs to CopperScript and reuses its lexer, strict parser, package
resolver, semantic compiler and electrical checks. CopperLib supplies source
definitions; consumer boards configure the selected compiler and locked inputs.

## Implementation plan and delivered first release

1. **Source frontend — delivered.** Add editor-only recovery around the strict
   parser. Retain complete sibling declarations and exact token ranges when a
   declaration is unfinished. Keep normal compilation strict. Convert source
   offsets to zero-based UTF-16 positions, including Unicode and CRLF documents.
2. **Workspace analysis — delivered.** Maintain versioned unsaved buffers and a
   source-derived index of local declarations and declared package exports. Feed
   all opened imported buffers through an explicit compiler `source_reader`
   hook. Resolve dependencies locked/offline; do not fetch, update locks, write
   source, or run placement/routing/native tools from editor analysis.
3. **Language features — delivered.** Report syntax, compiler and board ERC
   diagnostics; offer declaration/property/type/pin/port completions; show
   source documentation and declared electrical properties; navigate to local
   declarations and imported types, pins and module ports; provide an outline.
4. **Standard protocol — delivered.** Serve bounded JSON-RPC over stdio using
   LSP Content-Length framing, initialize/shutdown/exit, incremental or full
   document changes, save/close and watched-file notifications. Debounce
   semantic checks in one worker; discard obsolete workspace results.
5. **Editor integration and acceptance — delivered.** Extend the existing offline
   VS Code extension. Test actual stdio exchanges, providers, unsaved imports, locked failures,
   malformed frames, Unicode edits, recovery and stale result suppression.

The server is the `copper lsp` command of the `copper` binary (crate
`copper-lsp`); no separate installation is required. The small
VS Code client implements only the capabilities advertised by this server.
Other editors can use their existing standard LSP clients.

## Launch

Install `copper` on PATH (`cargo install --locked --path crates/copper`, or a
release binary), then run:

```sh
copper lsp
```

The command is a stdio protocol process, not an interactive terminal command.
An editor launches it and owns its lifetime. Logs go to stderr; stdout contains
protocol messages only. UTF-16 is the advertised position encoding.

For Neovim 0.11+, an example client configuration is:

```lua
vim.filetype.add({ extension = { copper = 'copper' } })
vim.lsp.config('copper', {
  cmd = { 'copper', 'lsp' },
  filetypes = { 'copper' },
  root_markers = { 'copper.mod', '.git' },
})
vim.lsp.enable('copper')
```

Use the editor's workspace file-watching facility to send
`workspace/didChangeWatchedFiles` for `.copper`, `copper.mod` and `copper.lock`.
Save notifications also invalidate the analysis cache. Missing dependencies or
locks are visible diagnostics; bootstrap them through the normal CLI workflow.

## VS Code

Build/install using [the extension README](../integrations/vscode/README.md).
The extension supplies a TextMate grammar for syntax highlighting, including
engineering units. It works independently of the compiler process; semantic
tokens remain follow-up work in the LSP.

Text `.copper` documents automatically activate the language tools in trusted
local workspaces. `copperscript.lsp.compilerCommand` is the argv array of the
`copper` binary, never a shell string. Its default `["copper"]` runs `copper`
from PATH (`[]` also selects it); an explicit array such as
`["/absolute/path/to/copper"]` selects another binary. The extension appends
`lsp` without using a shell.

Use **CopperScript: Show Setup** to inspect the selected command and
**CopperScript: Restart Language Server** after replacing the binary.
Startup failures offer setup guidance; a failing selected binary does not
silently select a different compiler. The
[extension release plan](extension-release-plan.md) covers packaging,
compatibility CI and Marketplace distribution.

Projects pinned to an older compiler must explicitly select an LSP-capable
binary, or disable the LSP with `copperscript.lsp.enabled`. An explicitly
selected newer compiler is editor guidance; it does not upgrade the project's
build pin. The mechanical editor is not part of the extension; run
`copper edit-mechanical BOARD.copper` for the browser editor.

Each workspace folder uses its configured compiler process. Open Copper buffers
are synchronized to the processes so unsaved local replacements/imports work
across folders. Only a document's owning folder publishes its diagnostics in VS
Code. Close discards that buffer and restores disk-backed import analysis.

## Current limits and follow-up plan

- Recovery retains complete top-level declarations; it does not synthesize
  valid semantics for malformed declarations. An unterminated string can consume
  subsequent source. Syntax errors suppress board ERC until the source parses.
- Library files get syntax diagnostics independently; complete semantic/ERC
  checks run through their consuming board. Built-in library pins have
  completions but no `.copper` definition locations.
- Completion offers a useful subset of properties and keywords, not every
  mechanical/constraint property or mode-dependent legal peripheral choice.
  Hovers show source declarations; inherited device-mode profiles are follow-up.
- ERC findings use the same locator as `copper check --json` (a finding's
  declaration and repeat occurrence pick the exact token: a duplicate net points
  at the duplicate). Findings without a precise source subject use the document
  start. Imported compiler errors attach their original location as related
  information.
- Quick fixes (code actions) apply machine-applicable replacements (`U3.SDAA` ->
  `U3.SDA`, `componnet` -> `component`, unknown constraint targets) and declare
  `no_connect` for every pin of an UNMARKED_PIN warning, inserted as the last
  board member with a TODO reason. Fix data travels in each diagnostic's `data`.
- The VS Code client restarts a crashed server with backoff (at most five times in
  five minutes) and shows its state in the status bar; a request timeout (30 s) is
  not retried automatically. Quick fixes are offered only while the text they
  were computed for is unchanged, and a `no_connect` insertion only if it parses.
- File URIs: VS Code's `file:///c%3A/...` drive encoding is decoded on Windows,
  and replies (diagnostics, definitions) use the URI the client opened, so files
  reached through a symlink, junction or `subst` drive work.
- Document analysis caches are invalidated conservatively across open documents.
  Verified dependency snapshots are reused while typing in a board, then
  discarded on import-buffer, save, manifest, lock or watched-file changes. Imports
  are indexed from the packages actually used; the server does not scan every
  library or build directory. Finer dependency invalidation, incremental
  parsing, background indexing and large-project latency measurements are next.
- Add scope-aware reference indexing before references/rename. Then add
  semantic tokens and comment-preserving formatting. These capabilities are
  deliberately not advertised by this release.
- Requests are served serially; cancellation notifications are accepted. The
  VS Code client suppresses canceled and superseded responses. Cooperative
  cancellation inside long package analysis remains follow-up work.

## Verification

```sh
cargo test -p copper-lsp
cargo build --release -p copper
COPPER_TEST_BIN=target/release/copper node --test tests/compiler-client.cjs tests/lsp-client.cjs
npm ci --prefix integrations/vscode/tools --ignore-scripts --no-audit --no-fund
node integrations/vscode/build.cjs
cd build/vscode-extension/extension
../../../integrations/vscode/tools/node_modules/.bin/vsce package --no-dependencies --out ../../copperscript-0.4.0.vsix
```

The Node tests exercise the actual stdio server and the VS Code provider adapter
with a host API fixture; they do not claim a visual Extension Development Host
test. Board ERC diagnostics are source checks, not hardware qualification.

Protocol reference: [LSP 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/).
